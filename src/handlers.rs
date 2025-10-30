use actix_web::{web, HttpResponse, Result};
use crate::server::Server;

pub async fn handle_browse(
    path: web::Path<String>,
    server: web::Data<Server>,
) -> Result<HttpResponse> {
    let file_infos = server.browse(&path).await?;
    Ok(HttpResponse::Ok().json(file_infos))
}

pub async fn handle_precache(
    path: web::Path<String>,
    server: web::Data<Server>,
) -> Result<HttpResponse> {
    server.start_precache(&path).await?;
    Ok(HttpResponse::Ok().json("Precaching started"))
}

pub async fn handle_cache_progress(
    path: web::Path<String>,
    server: web::Data<Server>,
) -> Result<HttpResponse> {
    let progress = server.get_cache_progress(&path).await?;
    Ok(HttpResponse::Ok().json(progress))
}