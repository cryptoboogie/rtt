use bytes::Bytes;
use http::{Method, Request, Uri};

const MAX_PATCH_SLOTS: usize = 8;
const MAX_BODY_SIZE: usize = 4096;

#[derive(Debug, Clone)]
struct PatchSlot {
    offset: usize,
    length: usize,
}

/// Fixed-capacity HTTP request template used by benchmark and executor scaffolding.
///
/// The body buffer is stored inline and copied into `Bytes` when a request is
/// built. Production CLOB order dispatch does not mutate signed payloads with
/// this type.
#[derive(Debug, Clone)]
pub struct RequestTemplate {
    method: Method,
    uri: Uri,
    headers: Vec<(String, String)>,
    body: [u8; MAX_BODY_SIZE],
    body_len: usize,
    patches: Vec<PatchSlot>,
}

impl RequestTemplate {
    pub fn new(method: Method, uri: Uri) -> Self {
        Self {
            method,
            uri,
            headers: Vec::new(),
            body: [0u8; MAX_BODY_SIZE],
            body_len: 0,
            patches: Vec::new(),
        }
    }

    pub fn add_header(&mut self, name: &str, value: &str) {
        self.headers.push((name.to_string(), value.to_string()));
    }

    pub fn set_body(&mut self, body: &[u8]) {
        assert!(body.len() <= MAX_BODY_SIZE, "body too large");
        self.body[..body.len()].copy_from_slice(body);
        self.body_len = body.len();
    }

    /// Register a patchable region in the body. Returns slot index.
    pub fn register_patch(&mut self, offset: usize, length: usize) -> usize {
        assert!(offset + length <= self.body_len, "patch out of bounds");
        assert!(self.patches.len() < MAX_PATCH_SLOTS, "too many patches");
        let idx = self.patches.len();
        self.patches.push(PatchSlot { offset, length });
        idx
    }

    /// Patch a registered slot with new value. Value must be exactly slot length.
    pub fn patch(&mut self, slot: usize, value: &[u8]) {
        let p = &self.patches[slot];
        assert_eq!(value.len(), p.length, "patch value length mismatch");
        self.body[p.offset..p.offset + p.length].copy_from_slice(value);
    }

    /// Build an HTTP request from the current template contents.
    pub fn build_request(&self) -> Request<Bytes> {
        let body = Bytes::copy_from_slice(&self.body[..self.body_len]);
        let mut builder = Request::builder()
            .method(self.method.clone())
            .uri(self.uri.clone());
        for (k, v) in &self.headers {
            builder = builder.header(k.as_str(), v.as_str());
        }
        builder.body(body).expect("failed to build request")
    }

    pub fn body_bytes(&self) -> &[u8] {
        &self.body[..self.body_len]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::polymarket::{CLOB_HOST, CLOB_ROOT_URL};

    #[test]
    fn create_template() {
        let tmpl = RequestTemplate::new(Method::GET, CLOB_ROOT_URL.parse().unwrap());
        assert_eq!(tmpl.body_len, 0);
    }

    #[test]
    fn set_body_and_read() {
        let mut tmpl = RequestTemplate::new(Method::POST, "/order".parse().unwrap());
        let body = b"{\"price\":\"0.45\"}";
        tmpl.set_body(body);
        assert_eq!(tmpl.body_bytes(), body);
    }

    #[test]
    fn register_and_patch() {
        let mut tmpl = RequestTemplate::new(Method::POST, "/order".parse().unwrap());
        let body = b"{\"price\":\"XXXX\"}";
        tmpl.set_body(body);
        let slot = tmpl.register_patch(10, 4); // "XXXX" at offset 10
        tmpl.patch(slot, b"0.75");
        assert_eq!(&tmpl.body_bytes()[10..14], b"0.75");
    }

    #[test]
    fn multiple_patches() {
        let mut tmpl = RequestTemplate::new(Method::POST, "/order".parse().unwrap());
        let body = b"{\"price\":\"XXXX\",\"size\":\"YYYY\"}";
        tmpl.set_body(body);
        let s1 = tmpl.register_patch(10, 4);
        let s2 = tmpl.register_patch(23, 4);
        tmpl.patch(s1, b"0.45");
        tmpl.patch(s2, b"1000");
        assert_eq!(&tmpl.body_bytes()[10..14], b"0.45");
        assert_eq!(&tmpl.body_bytes()[23..27], b"1000");
    }

    #[test]
    fn build_request_has_headers() {
        let mut tmpl = RequestTemplate::new(Method::GET, "/".parse().unwrap());
        tmpl.add_header("host", CLOB_HOST);
        tmpl.add_header("content-type", "application/json");
        let req = tmpl.build_request();
        assert_eq!(req.headers().get("host").unwrap(), CLOB_HOST);
        assert_eq!(
            req.headers().get("content-type").unwrap(),
            "application/json"
        );
    }

    #[test]
    fn build_request_with_patched_body() {
        let mut tmpl = RequestTemplate::new(Method::POST, "/order".parse().unwrap());
        tmpl.set_body(b"{\"price\":\"XXXX\"}");
        let slot = tmpl.register_patch(10, 4);
        tmpl.patch(slot, b"0.99");
        let req = tmpl.build_request();
        assert_eq!(req.body().as_ref(), b"{\"price\":\"0.99\"}");
    }
}
