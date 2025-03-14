use actix_web::{web, App, HttpServer, HttpResponse, Responder};
use clap::Parser;
use std::path::PathBuf;

mod models;
mod cache_manager;
mod directory_sizer;
mod server;
mod handlers;

use server::Server;
use handlers::{handle_browse, handle_precache, handle_cache_progress};

// Include the HTML file at compile time
const INDEX_HTML: &str = include_str!("../frontend/index.html");
const TAILWIND_CSS: &str = include_str!("../frontend/js/tailwindcss.js");
const REACT: &str = include_str!("../frontend/js/react.production.min.js");
const REACT_DOM: &str = include_str!("../frontend/js/react-dom.production.min.js");
const BABEL: &str = include_str!("../frontend/js/babel.min.js");


#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[arg(long)]
    mount: PathBuf,
    
    #[arg(long)]
    cache: PathBuf,
    
    #[arg(long, default_value = "1")]
    chunk: usize,
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
    
    if !args.mount.exists() || !args.cache.exists() {
        tracing::error!("Mount and cache paths must exist");
        std::process::exit(1);
    }
    
    let server = Server::new(
        args.mount,
        args.cache,
        args.chunk * 1024 * 1024,
    );
    let server_data = web::Data::new(server);
    
    println!("Starting server at http://127.0.0.1:8000");
    
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
                    .route("/cache-progress/{path:.*}", web::get().to(handle_cache_progress))
            )
            // serve js
            .route("/js/tailwindcss.js", web::get().to(|| async { HttpResponse::Ok().content_type("text/javascript").body(TAILWIND_CSS) }))
            .route("/js/react.production.min.js", web::get().to(|| async { HttpResponse::Ok().content_type("text/javascript").body(REACT) }))
            .route("/js/react-dom.production.min.js", web::get().to(|| async { HttpResponse::Ok().content_type("text/javascript").body(REACT_DOM) }))
            .route("/js/babel.min.js", web::get().to(|| async { HttpResponse::Ok().content_type("text/javascript").body(BABEL) }))
            // Serve index.html for all other routes
            .default_service(web::get().to(serve_index))
    })
    .bind("0.0.0.0:8000")?
    .run()
    .await
}
