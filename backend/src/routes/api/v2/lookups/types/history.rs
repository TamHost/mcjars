use super::State;
use utoipa_axum::{router::OpenApiRouter, routes};

mod get {
    use crate::routes::{ApiError, GetState};
    use axum::{extract::Path, http::StatusCode};
    use chrono::Datelike;
    use indexmap::IndexMap;
    use serde::{Deserialize, Serialize};
    use sqlx::Row;
    use utoipa::ToSchema;

    #[derive(ToSchema, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct TypeStats {
        day: i16,
        total: i64,
        unique_ips: i64,
    }

    #[derive(ToSchema, Serialize)]
    struct Response {
        success: bool,

        #[schema(inline)]
        types: IndexMap<String, Vec<TypeStats>>,
    }

    #[utoipa::path(get, path = "/{year}/{month}", responses(
        (status = OK, body = inline(Response)),
        (status = BAD_REQUEST, body = inline(ApiError)),
    ), params(
        (
            "year" = u16,
            description = "The year to get the version history for",
            minimum = 2024,
        ),
        (
            "month" = u8,
            description = "The month to get the version history for",
            minimum = 1,
            maximum = 12,
        ),
    ))]
    pub async fn route(
        state: GetState,
        Path((year, month)): Path<(u16, u8)>,
    ) -> (StatusCode, axum::Json<serde_json::Value>) {
        if year < 2024 || year > chrono::Utc::now().year() as u16 {
            return (
                StatusCode::BAD_REQUEST,
                axum::Json(ApiError::new(&["Invalid year"]).to_value()),
            );
        }

        if !(1..=12).contains(&month) {
            return (
                StatusCode::BAD_REQUEST,
                axum::Json(ApiError::new(&["Invalid month"]).to_value()),
            );
        }

        let start = chrono::NaiveDate::from_ymd_opt(year as i32, month as u32, 1).unwrap();
        let end = {
            let next_month = if month == 12 {
                chrono::NaiveDate::from_ymd_opt(year as i32 + 1, 1, 1).unwrap()
            } else {
                chrono::NaiveDate::from_ymd_opt(year as i32, month as u32 + 1, 1).unwrap()
            };

            next_month.pred_opt().unwrap()
        };

        let types = state
            .cache
            .cached(
                &format!("lookups::types::all::history::{}::{}", start, end),
                10800,
                || async {
                    let data = sqlx::query(
                        r#"
                        SELECT
                            search_type AS type,
                            day::smallint AS day,
                            SUM(total_requests)::bigint AS total,
                            SUM(unique_ips)::bigint AS unique_ips
                        FROM mv_requests_stats_daily
                        WHERE
                            request_type = 'builds'
                            AND search_type IS NOT NULL
                            AND date_only >= $1::date
                            AND date_only <= $2::date
                        GROUP BY day, search_type
                        ORDER BY day, SUM(total_requests) DESC
                        "#,
                    )
                    .bind(start)
                    .bind(end)
                    .fetch_all(state.database.read())
                    .await
                    .unwrap();

                    let mut types = IndexMap::new();
                    for row in &data {
                        let r#type = row.get::<String, _>("type");
                        if !types.contains_key(&r#type) {
                            let mut stats = Vec::with_capacity(end.day() as usize);

                            for i in 0..end.day() {
                                stats.push(TypeStats {
                                    day: i as i16 + 1,
                                    total: 0,
                                    unique_ips: 0,
                                });
                            }

                            types.insert(r#type, stats);
                        }
                    }

                    for row in data {
                        let r#type = row.get::<String, _>("type");
                        let day = row.get::<i16, _>("day") as usize - 1;

                        let entry = types.get_mut(&r#type).unwrap().get_mut(day).unwrap();
                        entry.total = row.get("total");
                        entry.unique_ips = row.get("unique_ips");
                    }

                    types
                },
            )
            .await;

        (
            StatusCode::OK,
            axum::Json(
                serde_json::to_value(&Response {
                    success: true,
                    types,
                })
                .unwrap(),
            ),
        )
    }
}

pub fn router(state: &State) -> OpenApiRouter<State> {
    OpenApiRouter::new()
        .routes(routes!(get::route))
        .with_state(state.clone())
}
