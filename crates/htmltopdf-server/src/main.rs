//! A small synchronous HTTP server around the htmltopdf engine.
//!
//! It is intentionally lightweight (one dependency, `tiny_http`, no async
//! runtime) and thread-pooled, so each request renders independently on a worker
//! thread — matching the engine's low-RAM / high-concurrency design.
//!
//! Endpoints:
//!   POST /render   body = HTML, returns application/pdf
//!   GET  /health   liveness check
//!   GET  /         usage help

use std::io::Read;
use std::sync::Arc;
use std::thread;

use htmltopdf::{Engine, FontSource, PageSize, RenderOptions};
use tiny_http::{Header, Request, Response, Server};

/// Maximum accepted request body (HTML) size.
const MAX_BODY: usize = 32 * 1024 * 1024;
const CT_TEXT: &str = "text/plain; charset=utf-8";
/// Keep a positive content box on the smallest supported paper dimension (A4
/// width). This also rejects `NaN` and infinities before they reach PDF output.
const MAX_MARGIN: f32 = PageSize::A4.width / 2.0;

const USAGE: &str = "\
htmltopdf-server

POST /render
  Request body: an HTML document.
  Response: application/pdf

  Optional query parameters:
    landscape=true     force landscape orientation
    margin=<points>    page margin in PDF points (e.g. margin=36)
    font=<path|family> embed a TrueType font by file path or system family name
    js=true            run the bounded pre-layout JavaScript stage (requires a
                       server build with `--features js`; rejected otherwise)

  Examples:
    curl -X POST http://127.0.0.1:8080/render \\
      -H 'Content-Type: text/html' \\
      --data-binary @invoice.html -o invoice.pdf

    curl -X POST 'http://127.0.0.1:8080/render?landscape=true&margin=36' \\
      --data-binary @report.html -o report.pdf

    curl -X POST 'http://127.0.0.1:8080/render?font=Georgia' \\
      --data-binary @invoice.html -o invoice.pdf

GET /health   -> 200 ok
";

fn main() {
    let addr = std::env::args()
        .nth(1)
        .or_else(|| std::env::var("HTMLTOPDF_ADDR").ok())
        .unwrap_or_else(|| "127.0.0.1:8080".to_string());

    let server = match Server::http(&addr) {
        Ok(server) => Arc::new(server),
        Err(error) => {
            eprintln!("htmltopdf-server: failed to bind {addr}: {error}");
            std::process::exit(1);
        }
    };

    // Worker thread count: HTMLTOPDF_WORKERS, else one per core.
    let workers = std::env::var("HTMLTOPDF_WORKERS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or_else(|| thread::available_parallelism().map(|n| n.get()).unwrap_or(4));

    eprintln!("htmltopdf-server listening on http://{addr} ({workers} worker threads)");
    eprintln!("  POST /render   body = HTML  -> application/pdf");
    eprintln!("  GET  /health   -> ok");
    eprintln!("  GET  /         -> usage");

    let mut handles = Vec::with_capacity(workers);
    for _ in 0..workers {
        let server = Arc::clone(&server);
        handles.push(thread::spawn(move || {
            for request in server.incoming_requests() {
                handle(request);
            }
        }));
    }

    for handle in handles {
        let _ = handle.join();
    }
}

type Handled = Result<(Vec<u8>, &'static str, Option<&'static str>), (u16, String)>;

fn handle(mut request: Request) {
    let method = request.method().as_str().to_string();
    let url = request.url().to_string();
    let path = url.split('?').next().unwrap_or("/").to_string();

    let result: Handled = match (method.as_str(), path.as_str()) {
        ("GET", "/health") => Ok((b"ok".to_vec(), CT_TEXT, None)),
        ("GET", "/") => Ok((USAGE.as_bytes().to_vec(), CT_TEXT, None)),
        ("POST", "/render") => render(&mut request, &url),
        ("GET", "/render") => Err((
            405,
            "use POST /render with an HTML document in the request body".to_string(),
        )),
        _ => Err((404, "not found — see GET / for usage".to_string())),
    };

    let outcome = match result {
        Ok((body, content_type, filename)) => {
            let mut response = Response::from_data(body)
                .with_header(header("Content-Type", content_type))
                .with_header(header("Access-Control-Allow-Origin", "*"));
            if let Some(name) = filename {
                response = response.with_header(header(
                    "Content-Disposition",
                    &format!("inline; filename=\"{name}\""),
                ));
            }
            request.respond(response)
        }
        Err((code, message)) => {
            let response = Response::from_string(message)
                .with_status_code(code)
                .with_header(header("Content-Type", CT_TEXT))
                .with_header(header("Access-Control-Allow-Origin", "*"));
            request.respond(response)
        }
    };

    if let Err(error) = outcome {
        eprintln!("htmltopdf-server: failed to send response: {error}");
    }
}

fn render(request: &mut Request, url: &str) -> Handled {
    if let Some(length) = content_length(request) {
        if length > MAX_BODY {
            return Err((413, format!("request body too large ({length} bytes; limit {MAX_BODY})")));
        }
    }

    let mut body = Vec::new();
    request
        .as_reader()
        .take((MAX_BODY as u64) + 1)
        .read_to_end(&mut body)
        .map_err(|error| (400, format!("failed to read request body: {error}")))?;

    if body.len() > MAX_BODY {
        return Err((413, format!("request body too large (limit {MAX_BODY} bytes)")));
    }

    let html = String::from_utf8(body).map_err(|_| (400, "request body is not valid UTF-8".to_string()))?;
    if html.trim().is_empty() {
        return Err((400, "empty request body; POST an HTML document to render".to_string()));
    }

    let (options, run_js) = options_from_query(url)?;

    // JavaScript is strictly opt-in per request (`js=true`) and only in builds
    // with the `js` feature — a request never pays any script cost otherwise.
    let rendered = if run_js {
        #[cfg(feature = "js")]
        {
            Engine::new().render_html_with_scripts(
                &html,
                options,
                &htmltopdf::BoaScriptEngine,
                &htmltopdf::ScriptLimits::default(),
            )
        }
        #[cfg(not(feature = "js"))]
        {
            return Err((
                400,
                "this build has no JavaScript support; rebuild with `--features js` to use js=true"
                    .to_string(),
            ));
        }
    } else {
        Engine::new().render_html(&html, options)
    };

    match rendered {
        Ok(pdf) => Ok((pdf, "application/pdf", Some("document.pdf"))),
        Err(error) => {
            let message = error.to_string();
            // An empty/blank document is a client error; everything else is 500.
            let code = if message.contains("does not contain renderable text") {
                400
            } else {
                500
            };
            Err((code, message))
        }
    }
}

fn options_from_query(url: &str) -> Result<(RenderOptions, bool), (u16, String)> {
    let query = url.split_once('?').map(|(_, q)| q).unwrap_or("");
    let mut options = RenderOptions::default();
    let mut run_js = false;

    for pair in query.split('&').filter(|part| !part.is_empty()) {
        let (key, raw_value) = pair.split_once('=').unwrap_or((pair, ""));
        let value = percent_decode(raw_value);

        match key {
            "js" if value == "true" || value == "1" => {
                run_js = true;
            }
            "landscape" if value == "true" || value == "1" => {
                options.page_size = PageSize::A4_LANDSCAPE;
            }
            "margin" => {
                let margin = value
                    .parse::<f32>()
                    .ok()
                    .filter(|margin| margin.is_finite() && *margin >= 0.0 && *margin < MAX_MARGIN)
                    .ok_or_else(|| {
                        (
                            400,
                            format!(
                                "margin must be a finite value from 0 (inclusive) to {MAX_MARGIN} (exclusive)"
                            ),
                        )
                    })?;
                options.margin = margin;
                options.margin_top = margin;
                options.margin_right = margin;
                options.margin_bottom = margin;
                options.margin_left = margin;
            }
            "font" if !value.is_empty() => {
                let source = if std::path::Path::new(&value).is_file() {
                    FontSource::Path(value.into())
                } else {
                    FontSource::Family(value)
                };
                options = options
                    .with_font(&source)
                    .map_err(|error| (400, format!("font error: {error}")))?;
            }
            _ => {}
        }
    }

    Ok((options, run_js))
}

fn content_length(request: &Request) -> Option<usize> {
    request
        .headers()
        .iter()
        .find(|header| header.field.equiv("Content-Length"))
        .and_then(|header| header.value.as_str().parse().ok())
}

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("valid header")
}

/// Minimal `application/x-www-form-urlencoded` decoding for query values.
fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;

    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                match (hex_digit(bytes[i + 1]), hex_digit(bytes[i + 2])) {
                    (Some(hi), Some(lo)) => {
                        out.push(hi * 16 + lo);
                        i += 3;
                    }
                    _ => {
                        out.push(b'%');
                        i += 1;
                    }
                }
            }
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }

    String::from_utf8_lossy(&out).into_owned()
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::options_from_query;

    #[test]
    fn rejects_non_finite_and_overlarge_margins() {
        for value in ["NaN", "inf", "-1", "298"] {
            let error = options_from_query(&format!("/render?margin={value}"))
                .expect_err("invalid margin must be rejected");
            assert_eq!(error.0, 400, "{value}");
        }
    }

    #[test]
    fn accepts_a_finite_margin_with_room_for_content() {
        let (options, _) = options_from_query("/render?margin=36").expect("valid margin");
        assert_eq!(options.margin_top, 36.0);
        assert_eq!(options.margin_left, 36.0);
    }
}
