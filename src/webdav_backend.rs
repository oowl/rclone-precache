use crate::storage_backend::{FileEntry, StorageBackend};
use async_trait::async_trait;
use bytes::Bytes;
use futures::Stream;
use reqwest::Client;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, ReadBuf};
use url::Url;

#[derive(Clone)]
pub struct WebDAVBackend {
    client: Client,
    base_url: Url,
    username: Option<String>,
    password: Option<String>,
}

impl WebDAVBackend {
    pub fn new(
        base_url: String,
        username: Option<String>,
        password: Option<String>,
    ) -> io::Result<Self> {
        let base_url = Url::parse(&base_url)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;

        let client = Client::builder()
            .build()
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

        Ok(Self {
            client,
            base_url,
            username,
            password,
        })
    }

    fn build_url(&self, path: &str) -> io::Result<Url> {
        let path = if path.starts_with('/') {
            &path[1..]
        } else {
            path
        };

        let path = self.base_url.path().trim_end_matches('/').to_string() + "/" + path;

        self.base_url
            .join(&path)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))
    }

    fn add_auth(&self, mut request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if let (Some(username), Some(password)) = (&self.username, &self.password) {
            request = request.basic_auth(username, Some(password));
        }
        request
    }

    async fn propfind(&self, path: &str, depth: u8) -> io::Result<String> {
        let url = self.build_url(path)?;

                println!("url built: {}", url.clone());
        let body = r#"<?xml version="1.0" encoding="utf-8" ?>
<D:propfind xmlns:D="DAV:">
  <D:prop>
    <D:displayname/>
    <D:getcontentlength/>
    <D:getlastmodified/>
    <D:resourcetype/>
  </D:prop>
</D:propfind>"#;

        let request = self.client.request(reqwest::Method::from_bytes(b"PROPFIND").unwrap(), url)
            .header("Depth", depth.to_string())
            .header("Content-Type", "application/xml")
            .body(body);

        let request = self.add_auth(request);


        println!("PROPFIND auth: {:?} {:?}", self.username, self.password);
        println!("PROPFIND request: {:?}", request);
        let response = request
            .send()
            .await
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

        println!("self base_url: {}", self.base_url);

        println!("PROPFIND response status: {:?}", response);

        if !response.status().is_success() {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                format!("WebDAV PROPFIND failed: {}", response.status()),
            ));
        }

        response
            .text()
            .await
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))
    }

    fn parse_propfind_response(&self, xml: &str, base_path: &str) -> io::Result<Vec<FileEntry>> {
        use quick_xml::events::Event;
        use quick_xml::Reader;
        
        let mut entries = Vec::new();
        let mut reader = Reader::from_str(xml);
        reader.trim_text(true);
        
        let mut buf = Vec::new();
        
        // Current entry being parsed
        let mut current_href: Option<String> = None;
        let mut current_is_collection = false;
        let mut current_size: Option<i64> = None;
        let mut current_modified: Option<f64> = None;
        
        // State tracking
        let mut in_response = false;
        let mut in_href = false;
        let mut _in_collection = false;
        let mut in_contentlength = false;
        let mut in_lastmodified = false;
        
        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                    let name = e.name();
                    let name_owned = name.local_name();
                    let local_name = name_owned.as_ref();
                    
                    match local_name {
                        b"response" => {
                            in_response = true;
                            // Reset current entry
                            current_href = None;
                            current_is_collection = false;
                            current_size = None;
                            current_modified = None;
                        }
                        b"href" if in_response => {
                            in_href = true;
                        }
                        b"collection" if in_response => {
                            _in_collection = true;
                            current_is_collection = true;
                        }
                        b"getcontentlength" if in_response => {
                            in_contentlength = true;
                        }
                        b"getlastmodified" if in_response => {
                            in_lastmodified = true;
                        }
                        _ => {}
                    }
                }
                Ok(Event::End(e)) => {
                    let name = e.name();
                    let name_owned = name.local_name();
                    let local_name = name_owned.as_ref();
                    
                    match local_name {
                        b"response" => {
                            in_response = false;
                            
                            // Process completed entry
                            if let Some(href) = current_href.take() {
                                if let Ok(entry) = self.process_webdav_entry(
                                    &href,
                                    base_path,
                                    current_is_collection,
                                    current_size,
                                    current_modified,
                                ) {
                                    if let Some(entry) = entry {
                                        entries.push(entry);
                                    }
                                }
                            }
                        }
                        b"href" => in_href = false,
                        b"collection" => _in_collection = false,
                        b"getcontentlength" => in_contentlength = false,
                        b"getlastmodified" => in_lastmodified = false,
                        _ => {}
                    }
                }
                Ok(Event::Text(e)) => {
                    if in_href {
                        if let Ok(text) = e.unescape() {
                            current_href = Some(text.to_string());
                        }
                    } else if in_contentlength {
                        if let Ok(text) = e.unescape() {
                            current_size = text.parse::<i64>().ok();
                        }
                    } else if in_lastmodified {
                        if let Ok(text) = e.unescape() {
                            // Parse RFC 2822 date
                            current_modified = chrono::DateTime::parse_from_rfc2822(&text)
                                .ok()
                                .map(|dt| dt.timestamp() as f64);
                        }
                    }
                }
                Ok(Event::Eof) => break,
                Err(e) => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("XML parse error: {}", e),
                    ));
                }
                _ => {}
            }
            buf.clear();
        }
        
        Ok(entries)
    }
    
    fn process_webdav_entry(
        &self,
        href: &str,
        base_path: &str,
        is_collection: bool,
        size: Option<i64>,
        modified_time: Option<f64>,
    ) -> io::Result<Option<FileEntry>> {
        // Decode URL-encoded href
        let decoded_href = urlencoding::decode(href)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?
            .to_string();
        
        // Normalize paths for comparison
        let normalized_base = base_path.trim_end_matches('/');
        let normalized_href = decoded_href.trim_end_matches('/');
        
        // Skip the base directory itself when listing
        if normalized_href == normalized_base 
            || normalized_href.is_empty() 
            || decoded_href == self.base_url.path() 
            || decoded_href == self.base_url.path().trim_end_matches('/') {
            return Ok(None);
        }
        
        // Extract the file/directory name from the href
        let name = normalized_href
            .rsplit('/')
            .next()
            .unwrap_or("")
            .to_string();
        
        if name.is_empty() {
            return Ok(None);
        }
        
        // Build the entry path
        let entry_path = if base_path.is_empty() || base_path == "/" {
            format!("/{}", name)
        } else {
            format!("{}/{}", base_path, name)
        };
        
        Ok(Some(FileEntry {
            name,
            path: entry_path,
            is_dir: is_collection,
            size: if !is_collection { size } else { None },
            modified_time: modified_time.unwrap_or(0.0),
        }))
    }
}

#[async_trait]
impl StorageBackend for WebDAVBackend {
    async fn list_dir(&self, path: &str) -> io::Result<Vec<FileEntry>> {
        let xml = self.propfind(path, 1).await?;
        println!("PROPFIND response XML: {}", xml);
        self.parse_propfind_response(&xml, path)
    }

    async fn metadata(&self, path: &str) -> io::Result<FileEntry> {
        let xml = self.propfind(path, 0).await?;
        let mut entries = self.parse_propfind_response(&xml, "")?;
        
        entries
            .pop()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "File not found"))
    }

    async fn open_file(&self, path: &str) -> io::Result<Box<dyn AsyncRead + Unpin + Send>> {
        let url = self.build_url(path)?;
        let request = self.client.get(url);
        let request = self.add_auth(request);

        let response = request
            .send()
            .await
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

        if !response.status().is_success() {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                format!("WebDAV GET failed: {}", response.status()),
            ));
        }

        let stream = response.bytes_stream();
        Ok(Box::new(StreamReader::new(stream)))
    }

    async fn file_size(&self, path: &str) -> io::Result<i64> {
        let entry = self.metadata(path).await?;
        entry
            .size
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Not a file"))
    }
}

// Helper struct to convert a Stream into AsyncRead
struct StreamReader {
    stream: Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>,
    current_chunk: Option<Bytes>,
    position: usize,
}

impl StreamReader {
    fn new(stream: impl Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static) -> Self {
        Self {
            stream: Box::pin(stream),
            current_chunk: None,
            position: 0,
        }
    }
}

impl AsyncRead for StreamReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        loop {
            if let Some(chunk) = &self.current_chunk {
                if self.position < chunk.len() {
                    let to_read = std::cmp::min(buf.remaining(), chunk.len() - self.position);
                    buf.put_slice(&chunk[self.position..self.position + to_read]);
                    self.position += to_read;
                    return Poll::Ready(Ok(()));
                } else {
                    self.current_chunk = None;
                    self.position = 0;
                }
            }

            match self.stream.as_mut().poll_next(cx) {
                Poll::Ready(Some(Ok(chunk))) => {
                    self.current_chunk = Some(chunk);
                    self.position = 0;
                }
                Poll::Ready(Some(Err(e))) => {
                    return Poll::Ready(Err(io::Error::new(io::ErrorKind::Other, e)));
                }
                Poll::Ready(None) => {
                    return Poll::Ready(Ok(()));
                }
                Poll::Pending => {
                    return Poll::Pending;
                }
            }
        }
    }
}
