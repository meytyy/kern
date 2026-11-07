//! LSP base protocol framing: `Content-Length` headers followed by JSON.
use std::io::{self, BufRead, Write};

pub fn read_message(r: &mut impl BufRead) -> io::Result<Option<Vec<u8>>> {
    let mut len: Option<usize> = None;
    let mut line = String::new();
    loop {
        line.clear();
        if r.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        let t = line.trim_end();
        if t.is_empty() {
            break;
        }
        if let Some((k, v)) = t.split_once(':')
            && k.eq_ignore_ascii_case("content-length")
        {
            len = v.trim().parse().ok();
        }
    }
    let len =
        len.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing Content-Length"))?;
    if len > 256 * 1024 * 1024 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "message too large",
        ));
    }
    let mut buf = vec![0; len];
    r.read_exact(&mut buf)?;
    Ok(Some(buf))
}

pub fn write_message(w: &mut impl Write, body: &[u8]) -> io::Result<()> {
    write!(w, "Content-Length: {}\r\n\r\n", body.len())?;
    w.write_all(body)?;
    w.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn framing_roundtrip_counts_bytes_not_chars() {
        let mut buf = vec![];
        write_message(&mut buf, "{\"a\":\"привет\"}".as_bytes()).unwrap();
        write_message(&mut buf, b"{}").unwrap();
        let mut r = io::Cursor::new(buf);
        assert_eq!(
            read_message(&mut r).unwrap().unwrap(),
            "{\"a\":\"привет\"}".as_bytes()
        );
        assert_eq!(read_message(&mut r).unwrap().unwrap(), b"{}");
        assert!(read_message(&mut r).unwrap().is_none());
    }

    #[test]
    fn extra_headers_and_case() {
        let raw = b"content-type: x\r\nCONTENT-LENGTH: 2\r\n\r\n{}";
        assert_eq!(
            read_message(&mut io::Cursor::new(&raw[..]))
                .unwrap()
                .unwrap(),
            b"{}"
        );
    }

    #[test]
    fn bad_input_is_an_error() {
        assert!(read_message(&mut io::Cursor::new(&b"X: 1\r\n\r\n{}"[..])).is_err());
        assert!(read_message(&mut io::Cursor::new(&b"Content-Length: 9\r\n\r\n{}"[..])).is_err());
    }
}
