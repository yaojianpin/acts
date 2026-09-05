use crate::objects::{AppError, RespData};
use acts::{Engine, Workflow, query::Query as ActsQuery};
use axum::{
    Json,
    extract::{Path, State},
    response::{IntoResponse, Result},
};
use serde::Deserialize;
use std::sync::Arc;
use validator::Validate;

#[derive(Debug, Deserialize)]
pub struct PacakgePublish {
    pub fmt: String,
    pub model: serde_json::Value,
    pub view: serde_json::Value,
}

#[derive(Debug, Deserialize)]
pub struct PacakgeGet {
    pub fmt: String,
    pub id: String,
}

#[derive(Debug, Deserialize)]
pub struct PacakgeId {
    pub id: String,
}

#[derive(Debug, Deserialize)]
pub struct PacakgeStart {
    pub mode: String,
    pub id: Option<String>,
    pub fmt: Option<String>,
    pub model: Option<serde_json::Value>,
    pub options: Option<acts::Vars>,
}

pub async fn deploy(
    State(state): State<Arc<Engine>>,
    Json(req): Json<PacakgePublish>,
) -> Result<impl IntoResponse, AppError> {
    let workflow = match req.fmt.as_str() {
        "json" => Workflow::from_json(&req.model.to_string())?,
        "ymal" | "yml" => Workflow::from_yml(&req.model.to_string())?,
        _ => return Err("fmt is not correct, it should be one of 'json' or 'ymal'".into()),
    };
    let ret = state
        .executor()
        .model()
        .deploy(&workflow, Some(&req.view))
        .await?;
    Ok(RespData::ok(ret))
}

pub async fn get(
    State(state): State<Arc<Engine>>,
    Json(req): Json<PacakgeGet>,
) -> Result<impl IntoResponse, AppError> {
    let ret = state.executor().model().get(&req.id, &req.fmt).await?;
    Ok(RespData::ok(ret))
}

pub async fn rm(
    State(state): State<Arc<Engine>>,
    Json(req): Json<PacakgeId>,
) -> Result<impl IntoResponse, AppError> {
    let ret = state.executor().model().rm(&req.id).await?;
    Ok(RespData::ok(ret))
}

pub async fn list(
    State(state): State<Arc<Engine>>,
    Json(req): Json<ActsQuery>,
) -> Result<impl IntoResponse, AppError> {
    let ret = state.executor().model().list(&req).await?;
    Ok(RespData::ok(ret))
}

pub async fn proc_start(
    State(state): State<Arc<Engine>>,
    Json(data): Json<PacakgeStart>,
) -> Result<impl IntoResponse, AppError> {
    let options = data.options.unwrap_or_default();
    let fmt = data.fmt.ok_or("fmt is required")?;
    let ret = match data.mode.as_str() {
        "model" => {
            let model = data.model.ok_or("model is required")?;
            state
                .executor()
                .proc()
                .start_from_model(&model.to_string(), &fmt, options)
                .await?
        }
        "id" => {
            let id = data.id.ok_or("id is required")?;
            state.executor().proc().start(&id, options).await?
        }
        _ => return Err("mode is not correct, it should be one of 'model' or 'id'".into()),
    };
    Ok(RespData::ok(ret))
}

#[derive(Debug, Validate, Deserialize)]
pub struct PackageParams {
    pub catalog: Option<String>,
    pub search: Option<String>,
    pub order: Option<String>,
    #[validate(range(min = 1))]
    pub size: Option<usize>,
    #[validate(range(min = 1))]
    pub page: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct PackageIdRequest {
    pub id: String,
}

pub async fn pack_list(
    State(state): State<Arc<Engine>>,
    Json(params): Json<PackageParams>,
) -> Result<impl IntoResponse, AppError> {
    params.validate()?;
    let mut query = ActsQuery::new();
    let mut filter = acts::query::Filter::and();
    if let Some(search) = &params.search {
        filter = filter.expr(acts::query::Expr::matches("name", search));
    }
    if let Some(catalog) = &params.catalog {
        filter = filter.expr(acts::query::Expr::eq("catalog", catalog));
    }
    if let Some(order) = &params.order {
        match order.as_str() {
            "desc" => query = query.order("version", acts::query::Sort::Desc),
            "asc" => query = query.order("version", acts::query::Sort::Asc),
            _ => (),
        }
    }
    if let Some(size) = params.size {
        query = query.limit(size);
        if let Some(page) = params.page
            && page > 0
        {
            query = query.offset((page - 1) * size);
        }
    }
    query = query.filter(filter);
    let ret = state.executor().pack().list(&query).await?;
    Ok(RespData::ok(ret))
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Catalog {
    pub id: String,
    pub icon: String,
    pub name: String,
    pub desc: String,
}

pub async fn pack_catalogs() -> Result<impl IntoResponse, AppError> {
    let catalogs = vec![
        Catalog {
            id: "core".to_string(),
            name: "Core".to_string(),
            icon: r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="4" y="4" width="16" height="16" rx="2"/><rect x="9" y="9" width="6" height="6" rx="1"/><path d="M15 2v2"/><path d="M15 20v2"/><path d="M2 15h2"/><path d="M2 9h2"/><path d="M20 15h2"/><path d="M20 9h2"/><path d="M9 2v2"/><path d="M9 20v2"/></svg>"#.to_string(),
            desc: "The basic packages from acts".to_string(),
        },
        Catalog {
            id: "transform".to_string(),
            name: "Transform".to_string(),
            icon: r#"<svg viewBox="0 0 1024 1024" xmlns="http://www.w3.org/2000/svg" fill="currentColor" width="24" height="24"><path d="M108.544 658.005c-21.973-23.637-6.912-60.416 24.15-64.682l5.973-0.427h746.709c22.357 0 40.49 17.493 40.49 39.04a39.552 39.552 0 0 1-34.986 38.699l-5.504 0.341-656.043 0.043 131.115 140.885a37.973 37.973 0 0 1 1.408 50.517l-4.523 4.608a41.643 41.643 0 0 1-52.394 1.366l-4.779-4.352-191.573-206.08z m807.85-292.01c20.011 23.637 6.273 60.416-21.973 64.682l-5.461 0.427H134.955c-20.31 0-36.822-17.493-36.822-39.04 0-19.755 13.867-36.096 31.83-38.699l4.992-0.341 671.488-0.043-119.339-140.928a40.79 40.79 0 0 1-1.28-50.474l4.139-4.608a35.285 35.285 0 0 1 47.658-1.366l4.352 4.352 174.422 206.08z"></path></svg>"#.to_string(),
            desc: "Packages to transform data".to_string(),
        },
        Catalog {
            id: "ai".to_string(),
            name: "AI".to_string(),
            icon: r#"<svg viewBox="0 0 1024 1024" xmlns="http://www.w3.org/2000/svg" fill="currentColor" width="24" height="24"><path d="M511.285 708.988H248.694L191.85 916.88H27.574l259.373-809.76h190.375l260.446 809.76H568.129l-56.844-207.892z m-35.036-125.843l-24.132-88.663c-25.204-84.194-47.013-177.325-71.145-264.915h-4.29c-20.735 88.662-44.867 180.543-69 264.915l-24.131 88.663h192.698zM833.044 107.12h161.952v809.76H833.044V107.12z"></path></svg>"#.to_string(),
            desc: "Packages to intergrate AI model".to_string(),
        },
        Catalog {
            id: "form".to_string(),
            name: "Form".to_string(),
            icon: r#"<svg viewBox="0 0 1024 1024" xmlns="http://www.w3.org/2000/svg" fill="currentColor" width="24" height="24"><path d="M696.32 413.76h-352a32 32 0 1 0 0 64h352a32 32 0 0 0 0-64z m0 192h-352a32 32 0 1 0 0 64h352a32 32 0 0 0 0-64z"></path><path d="M824.32 29.76h-608a96 96 0 0 0-96 96v768a96 96 0 0 0 96 96h608a96 96 0 0 0 96-96v-768a96 96 0 0 0-96-96z m-480 64h352v96h-352z m512 800a32 32 0 0 1-32 32h-608a32 32 0 0 1-32-32v-768a32 32 0 0 1 32-32h64v96a64 64 0 0 0 64 64h352a64 64 0 0 0 64-64v-96h64a32 32 0 0 1 32 32z"></path></svg>"#.to_string(),
            desc: "Packages to create form submission".to_string(),
        },
        Catalog {
            id: "app".to_string(),
            name: "App".to_string(),
            icon: r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="currentColor" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect width="7" height="7" x="3" y="3" rx="1"/><rect width="7" height="7" x="14" y="3" rx="1"/><rect width="7" height="7" x="14" y="14" rx="1"/><rect width="7" height="7" x="3" y="14" rx="1"/></svg>"#.to_string(),
            desc: "Packages from apps".to_string(),
        },
    ];
    Ok(RespData::ok(catalogs))
}

pub async fn pack_get(
    State(state): State<Arc<Engine>>,
    Json(package): Json<PackageIdRequest>,
) -> Result<impl IntoResponse, AppError> {
    let ret = state.executor().pack().get(&package.id).await?;
    Ok(RespData::ok(ret))
}

/// Fire a deployed trigger from an HTTP POST whose JSON body becomes the
/// trigger payload — a webhook-style URL trigger is just a `manual` trigger
/// reached over HTTP.
///
/// `event_id` is `{model-id}:{trigger-id}`; the engine answers with the same
/// result as `executor.evt().start()`.
pub async fn hook(
    State(state): State<Arc<Engine>>,
    Path(event_id): Path<String>,
    body: Option<Json<serde_json::Value>>,
) -> Result<impl IntoResponse, AppError> {
    let payload = body.map(|Json(v)| v).unwrap_or(serde_json::Value::Null);
    let ret = state.executor().evt().start(&event_id, &payload).await?;
    Ok(RespData::ok(ret))
}
