//! Container paths and the relative URLs (hrefs) that point into them.
//!
//! OCF paths are case-sensitive, `/`-separated and rooted at the container.
//! Hrefs are URL references: percent-encoded, relative to the referring
//! file, possibly with a fragment. Nothing may resolve outside the root.

/// An entry name as stored in the ZIP is safe to use as a container path.
#[must_use]
pub fn is_safe_entry(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('/')
        && !name.contains('\\')
        && !name.contains('\0')
        && !name.split('/').any(|seg| seg == ".." || seg == ".")
        && !name.contains(':')
}

/// Directory part of a container path (`"OEBPS/text/c1.xhtml"` → `"OEBPS/text"`).
#[must_use]
pub fn dir_of(path: &str) -> &str {
    path.rfind('/').map_or("", |i| &path[..i])
}

/// Decodes `%XX` escapes; `None` on a malformed escape or invalid UTF-8.
#[must_use]
pub fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = bytes.get(i + 1..i + 3)?;
            let hex = core::str::from_utf8(hex).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// A resolved reference: container path plus optional fragment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub path: String,
    pub fragment: Option<String>,
}

/// Resolves `href` against the directory `base_dir`. `None` for absolute
/// URLs (`scheme:`), paths that climb above the root, and bad escapes.
/// An href that is only a fragment (`#n1`) resolves to `self_path`.
#[must_use]
pub fn resolve(base_dir: &str, self_path: &str, href: &str) -> Option<Target> {
    let (raw, fragment) = match href.split_once('#') {
        Some((p, f)) => (p, Some(percent_decode(f)?)),
        None => (href, None),
    };
    let raw = raw.split_once('?').map_or(raw, |(p, _)| p);
    if raw.is_empty() {
        return Some(Target {
            path: self_path.to_owned(),
            fragment,
        });
    }
    if raw.starts_with('/') || raw.contains(':') {
        return None;
    }
    let decoded = percent_decode(raw)?;
    let mut segments: Vec<&str> = if base_dir.is_empty() {
        Vec::new()
    } else {
        base_dir.split('/').collect()
    };
    for seg in decoded.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                segments.pop()?;
            }
            s => segments.push(s),
        }
    }
    let path = segments.join("/");
    is_safe_entry(&path).then_some(Target { path, fragment })
}

/// Whether an href is an absolute URL (external link).
#[must_use]
pub fn is_external(href: &str) -> bool {
    href.split_once(':').is_some_and(|(scheme, _)| {
        !scheme.is_empty()
            && scheme
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.'))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(base: &str, href: &str) -> Option<(String, Option<String>)> {
        resolve(base, "OEBPS/text/self.xhtml", href).map(|t| (t.path, t.fragment))
    }

    #[test]
    fn resolves_relative_hrefs() {
        assert_eq!(
            r("OEBPS", "text/c1.xhtml"),
            Some(("OEBPS/text/c1.xhtml".into(), None))
        );
        assert_eq!(
            r("OEBPS/text", "../images/fig%201.png"),
            Some(("OEBPS/images/fig 1.png".into(), None))
        );
        assert_eq!(
            r("OEBPS/text", "c2.xhtml#n%31"),
            Some(("OEBPS/text/c2.xhtml".into(), Some("n1".into())))
        );
        assert_eq!(
            r("OEBPS/text", "#n1"),
            Some(("OEBPS/text/self.xhtml".into(), Some("n1".into())))
        );
    }

    #[test]
    fn refuses_escapes_and_urls() {
        assert_eq!(r("OEBPS", "../../etc/passwd"), None);
        assert_eq!(r("", "../x"), None);
        assert_eq!(r("OEBPS", "/abs.xhtml"), None);
        assert_eq!(r("OEBPS", "https://example.com/"), None);
        assert_eq!(r("OEBPS", "bad%zz"), None);
        assert!(is_external("https://example.com/"));
        assert!(is_external("mailto:x@y"));
        assert!(!is_external("text/c1.xhtml"));
    }

    #[test]
    fn entry_names() {
        assert!(is_safe_entry("OEBPS/content.opf"));
        for bad in ["/etc", "a/../b", "a\\b", "", "c:/x", "./a"] {
            assert!(!is_safe_entry(bad), "{bad}");
        }
    }
}
