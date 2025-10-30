use actix_web::{web, App, HttpResponse, HttpServer, Responder};
use clap::Parser;
use std::path::PathBuf;

mod cache_manager;
mod directory_sizer;
mod handlers;
mod models;
mod server;
mod storage_backend;
mod local_backend;
mod webdav_backend;

use handlers::{handle_browse, handle_cache_progress, handle_precache};
use server::Server;
use storage_backend::StorageBackend;
use local_backend::LocalFileSystem;
use webdav_backend::WebDAVBackend;

// Include the HTML file at compile time
const INDEX_HTML: &str = include_str!("../frontend/index.html");
const TAILWIND_CSS: &str = include_str!("../frontend/js/tailwindcss.js");
const REACT: &str = include_str!("../frontend/js/react.production.min.js");
const REACT_DOM: &str = include_str!("../frontend/js/react-dom.production.min.js");
const BABEL: &str = include_str!("../frontend/js/babel.min.js");

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Type of mount: local or webdav
    #[arg(long, default_value = "local")]
    mount_type: String,

    /// Mount path (for local filesystem)
    #[arg(long)]
    mount: Option<PathBuf>,

    /// WebDAV server URL (for webdav)
    #[arg(long)]
    webdav_url: Option<String>,

    /// WebDAV username
    #[arg(long)]
    webdav_username: Option<String>,

    /// WebDAV password
    #[arg(long)]
    webdav_password: Option<String>,

    /// Cache directory path
    #[arg(long)]
    cache: PathBuf,

    /// Chunk size in MB
    #[arg(long, default_value = "1")]
    chunk: usize,

    /// Number of cache threads
    #[arg(long, default_value = "2")]
    threads: usize,
}

// Handler for serving the index.html
async fn serve_index() -> impl Responder {
    HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(INDEX_HTML)
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    tracing_subscriber::fmt::init();

    let args = Args::parse();

    if !args.cache.exists() {
        tracing::error!("Cache path must exist");
        std::process::exit(1);
    }

    // Create storage backend based on mount type
    let storage_backend: Box<dyn StorageBackend> = match args.mount_type.as_str() {
        "local" => {
            let mount_path = args.mount.as_ref().ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "--mount is required for local mount type",
                )
            })?;

            if !mount_path.exists() {
                tracing::error!("Mount path must exist");
                std::process::exit(1);
            }

            Box::new(LocalFileSystem::new(mount_path.clone()))
        }
        "webdav" => {
            let webdav_url = args.webdav_url.as_ref().ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "--webdav-url is required for webdav mount type",
                )
            })?;

            Box::new(WebDAVBackend::new(
                webdav_url.clone(),
                args.webdav_username,
                args.webdav_password,
            )?)
        }
        _ => {
            tracing::error!("Invalid mount type. Use 'local' or 'webdav'");
            std::process::exit(1);
        }
    };

    let server = Server::new(
        storage_backend,
        args.cache,
        args.chunk * 1024 * 1024,
        args.threads,
    );
    let server_data = web::Data::new(server);

    println!("Starting server at http://127.0.0.1:8000");
    println!(
        "Cache using {} threads, {}MB chunk",
        args.threads, args.chunk
    );

    HttpServer::new(move || {
        let cors = actix_cors::Cors::default()
            .allow_any_origin()
            .allow_any_method()
            .allow_any_header();

        App::new()
            .wrap(cors)
            .app_data(server_data.clone())
            .service(
                web::scope("/api")
                    .route("/browse/{path:.*}", web::get().to(handle_browse))
                    .route("/precache/{path:.*}", web::post().to(handle_precache))
                    .route(
                        "/cache-progress/{path:.*}",
                        web::get().to(handle_cache_progress),
                    ),
            )
            // serve js
            .route(
                "/js/tailwindcss.js",
                web::get().to(|| async {
                    HttpResponse::Ok()
                        .content_type("text/javascript")
                        .body(TAILWIND_CSS)
                }),
            )
            .route(
                "/js/react.production.min.js",
                web::get().to(|| async {
                    HttpResponse::Ok()
                        .content_type("text/javascript")
                        .body(REACT)
                }),
            )
            .route(
                "/js/react-dom.production.min.js",
                web::get().to(|| async {
                    HttpResponse::Ok()
                        .content_type("text/javascript")
                        .body(REACT_DOM)
                }),
            )
            .route(
                "/js/babel.min.js",
                web::get().to(|| async {
                    HttpResponse::Ok()
                        .content_type("text/javascript")
                        .body(BABEL)
                }),
            )
            // Serve index.html for all other routes
            .default_service(web::get().to(serve_index))
    })
    .bind("0.0.0.0:8000")?
    .run()
    .await
}
