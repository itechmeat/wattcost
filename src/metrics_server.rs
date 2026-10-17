//! A minimal HTTP/1.1 server for `GET /metrics`, one connection at a time.

use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use chrono::Local;
use tracing::debug;

use crate::metrics::{CONTENT_TYPE, Metrics};
use crate::tariff::ClockTime;

/// Bounds how long one slow or idle client can hold the single connection slot.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_HEAD: usize = 8 * 1024;

fn serve(listener: &TcpListener, metrics: &Mutex<Metrics>) {
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                if let Err(error) = respond(stream, metrics) {
                    debug!("metrics request failed: {error}");
                }
            }
            Err(error) => debug!("metrics connection failed: {error}"),
        }
    }
}

/// Answers requests on a background thread for as long as the process runs.
pub fn spawn(listener: TcpListener, metrics: Arc<Mutex<Metrics>>) -> io::Result<()> {
    std::thread::Builder::new()
        .name("metrics".to_owned())
        .spawn(move || serve(&listener, &metrics))
        .map(drop)
}

fn respond(mut stream: TcpStream, metrics: &Mutex<Metrics>) -> io::Result<()> {
    let head = read_head(&mut stream)?;
    let mut parts = head.split(|&byte| byte == b' ');
    let method = parts.next().unwrap_or_default();
    let target = parts.next().unwrap_or_default();
    let path = target
        .split(|&byte| byte == b'?')
        .next()
        .unwrap_or_default();
    let (status, body) = match (method, path) {
        (b"GET" | b"HEAD", b"/metrics") => {
            let text = metrics
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .render(ClockTime::of(&Local::now()));
            ("200 OK", text)
        }
        (_, b"/metrics") => ("405 Method Not Allowed", "Method Not Allowed\n".to_owned()),
        _ => ("404 Not Found", "Not Found\n".to_owned()),
    };
    let content_type = if status.starts_with("200") {
        CONTENT_TYPE
    } else {
        "text/plain; charset=utf-8"
    };
    let allow = if status.starts_with("405") {
        "Allow: GET, HEAD\r\n"
    } else {
        ""
    };
    let mut response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n{allow}Connection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    if method != b"HEAD" {
        response.extend_from_slice(body.as_bytes());
    }
    stream.write_all(&response)?;
    stream.flush()
}

/// Reads the request line and headers; returns the request line.
fn read_head(stream: &mut TcpStream) -> io::Result<Vec<u8>> {
    let deadline = Instant::now() + REQUEST_TIMEOUT;
    let mut head = Vec::new();
    let mut buffer = [0; 1024];
    while !head.windows(4).any(|window| window == b"\r\n\r\n") {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() || head.len() > MAX_HEAD {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "request head too slow or too large",
            ));
        }
        stream.set_read_timeout(Some(left))?;
        stream.set_write_timeout(Some(REQUEST_TIMEOUT))?;
        match stream.read(&mut buffer)? {
            0 => break,
            read => head.extend_from_slice(&buffer[..read]),
        }
    }
    let line_end = head
        .windows(2)
        .position(|window| window == b"\r\n")
        .unwrap_or(head.len());
    head.truncate(line_end);
    Ok(head)
}
