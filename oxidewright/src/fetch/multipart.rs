//! `multipart/form-data` encoding (Playwright's `server/formData.ts`).

/// A file sent in a multipart form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilePayload {
    /// File name.
    pub name: String,
    /// MIME type; guessed from the name when `None`.
    pub mime_type: Option<String>,
    /// Content.
    pub buffer: Vec<u8>,
}

impl FilePayload {
    /// A file with a name, MIME type and content.
    pub fn new(
        name: impl Into<String>,
        mime_type: impl Into<String>,
        buffer: impl Into<Vec<u8>>,
    ) -> Self {
        Self {
            name: name.into(),
            mime_type: Some(mime_type.into()),
            buffer: buffer.into(),
        }
    }
}

/// The value of a multipart field: text or a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MultipartValue {
    /// A text field.
    Text(String),
    /// A file field.
    File(FilePayload),
}

impl From<&str> for MultipartValue {
    fn from(v: &str) -> Self {
        MultipartValue::Text(v.to_string())
    }
}

impl From<String> for MultipartValue {
    fn from(v: String) -> Self {
        MultipartValue::Text(v)
    }
}

impl From<FilePayload> for MultipartValue {
    fn from(v: FilePayload) -> Self {
        MultipartValue::File(v)
    }
}

macro_rules! text_from {
    ($($t:ty),*) => {$(
        impl From<$t> for MultipartValue {
            fn from(v: $t) -> Self {
                MultipartValue::Text(v.to_string())
            }
        }
    )*};
}
text_from!(bool, i32, i64, u32, u64, f64);

/// MIME type of a file name (`mime.getType`), for the common extensions.
pub(crate) fn mime_type_for(name: &str) -> Option<&'static str> {
    let ext = name.rsplit('.').next()?.to_ascii_lowercase();
    if !name.contains('.') {
        return None;
    }
    Some(match ext.as_str() {
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "js" | "mjs" => "text/javascript",
        "json" => "application/json",
        "txt" => "text/plain",
        "csv" => "text/csv",
        "xml" => "application/xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "ico" => "image/vnd.microsoft.icon",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "gz" => "application/gzip",
        "wasm" => "application/wasm",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        _ => return None,
    })
}

fn escape_name(name: &str) -> String {
    name.replace('\r', "%0D")
        .replace('\n', "%0A")
        .replace('"', "%22")
}

fn normalize_line_endings(name: &str) -> String {
    name.replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace('\n', "\r\n")
}

/// `generateUniqueBoundaryString` (WebKit style).
fn boundary() -> String {
    const MAP: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789AB";
    let suffix: String = (0..16)
        .map(|_| MAP[fastrand::usize(..MAP.len())] as char)
        .collect();
    format!("----WebKitFormBoundary{suffix}")
}

/// Encode the fields; returns the `content-type` header and the body.
pub(crate) fn encode(fields: &[(String, MultipartValue)]) -> (String, Vec<u8>) {
    let boundary = boundary();
    let mut out = Vec::new();
    for (name, value) in fields {
        out.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        out.extend_from_slice(
            format!(
                "content-disposition: form-data; name=\"{}\"",
                escape_name(&normalize_line_endings(name))
            )
            .as_bytes(),
        );
        match value {
            MultipartValue::Text(text) => {
                out.extend_from_slice(b"\r\n\r\n");
                out.extend_from_slice(text.as_bytes());
            }
            MultipartValue::File(file) => {
                let mime = file
                    .mime_type
                    .clone()
                    .or_else(|| mime_type_for(&file.name).map(str::to_string))
                    .unwrap_or_else(|| "application/octet-stream".to_string());
                out.extend_from_slice(
                    format!(
                        "; filename=\"{}\"\r\ncontent-type: {mime}",
                        escape_name(&file.name)
                    )
                    .as_bytes(),
                );
                out.extend_from_slice(b"\r\n\r\n");
                out.extend_from_slice(&file.buffer);
            }
        }
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_fields_and_files() {
        let (content_type, body) = encode(&[
            ("a\nb".into(), "x\"y".into()),
            (
                "file".into(),
                MultipartValue::File(FilePayload {
                    name: "f.json".into(),
                    mime_type: None,
                    buffer: b"{}".to_vec(),
                }),
            ),
        ]);
        let boundary = content_type.split("boundary=").nth(1).unwrap();
        assert!(boundary.starts_with("----WebKitFormBoundary"));
        assert_eq!(boundary.len(), "----WebKitFormBoundary".len() + 16);
        let text = String::from_utf8(body).unwrap();
        assert!(text.contains("content-disposition: form-data; name=\"a%0D%0Ab\"\r\n\r\nx\"y\r\n"));
        assert!(text.contains(
            "name=\"file\"; filename=\"f.json\"\r\ncontent-type: application/json\r\n\r\n{}\r\n"
        ));
        assert!(text.ends_with(&format!("--{boundary}--\r\n")));
    }
}
