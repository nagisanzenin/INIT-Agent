//! A response line is at most 1 MiB (kernel/initctl.md §3.1). When a result would exceed it,
//! the kernel truncates arrays and adds `"truncated": true`; the full data stays in the job
//! record and report files (§5).

use init_proto::envelope::MAX_RESPONSE_LINE;
use init_proto::errors::ErrorCode;
use init_proto::{ErrorBody, Response};
use serde_json::Value;

fn size(v: &Value) -> usize {
    serde_json::to_vec(v).map(|b| b.len()).unwrap_or(usize::MAX)
}

/// The largest array (≥ 1 element) or long string inside `v`, by serialized size. Only
/// candidates are measured; a measured candidate's insides are not searched.
fn largest<'a>(v: &'a mut Value, best: &mut Option<(usize, &'a mut Value)>) {
    let candidate = match v {
        Value::Array(a) => !a.is_empty(),
        Value::String(t) => t.len() > 1024,
        _ => false,
    };
    if candidate {
        let s = size(v);
        if best.as_ref().is_none_or(|(b, _)| s > *b) {
            *best = Some((s, v));
        }
        return;
    }
    if let Value::Object(m) = v {
        m.values_mut().for_each(|x| largest(x, best));
    }
}

fn halve(v: &mut Value) {
    match v {
        Value::Array(a) => {
            let n = a.len() / 2;
            a.truncate(n);
        }
        Value::String(s) => {
            let mut n = s.len() / 2;
            while !s.is_char_boundary(n) {
                n -= 1;
            }
            s.truncate(n);
        }
        _ => {}
    }
}

/// Make `resp` fit in one 1 MiB line.
pub fn fit(resp: Response) -> Response {
    let line_len = resp.to_line().len();
    if line_len <= MAX_RESPONSE_LINE {
        return resp;
    }
    let Response { id, result } = resp;
    let Ok(mut v) = result else {
        // Errors are small by construction (one-line message, short details).
        return Response::err(id, ErrorBody::new(ErrorCode::Internal, "error response too large"));
    };
    for _ in 0..256 {
        if let Value::Object(m) = &mut v {
            m.insert("truncated".into(), Value::Bool(true));
        }
        // Headroom for the envelope.
        if size(&v) + 256 <= MAX_RESPONSE_LINE {
            return Response::ok_value(id.unwrap_or_default(), v);
        }
        let mut best = None;
        largest(&mut v, &mut best);
        match best {
            Some((_, node)) => halve(node),
            None => break,
        }
    }
    Response::err(id, ErrorBody::new(ErrorCode::Internal, "response too large to send; see the job record"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn small_untouched_large_truncated() {
        let r = Response::ok_value("1", json!({"a": [1, 2, 3]}));
        assert_eq!(fit(r.clone()), r);
        let big: Vec<Value> = (0..30_000).map(|i| json!({"i": i, "s": "x".repeat(40)})).collect();
        let r = fit(Response::ok_value("1", json!({"jobs": big, "n": 1})));
        assert!(r.to_line().len() <= MAX_RESPONSE_LINE);
        let v = r.result.unwrap();
        assert_eq!(v["truncated"], json!(true));
        assert!(!v["jobs"].as_array().unwrap().is_empty());
        assert_eq!(v["n"], json!(1));
        let r = fit(Response::ok_value("1", json!({"blob": "é".repeat(800_000)})));
        assert!(r.to_line().len() <= MAX_RESPONSE_LINE && r.is_ok());
    }
}
