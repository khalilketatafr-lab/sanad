//! OCF: the ZIP container (EPUB 3.3 §4).
//!
//! Everything a publisher's archive can do to the ingest host is bounded
//! here, before any XML is read:
//!
//! - entry names that escape the root (absolute, `..`, `\`, drive letters);
//! - ZIP bombs: entry count, per-entry and total inflated size, and
//!   inflation ratio, checked against the declared sizes *and* enforced while
//!   inflating (a lying header cannot get past `take`);
//! - encrypted entries and `encryption.xml` other than font obfuscation:
//!   a DRM-protected file cannot be typeset and is refused, not guessed at;
//! - compression methods other than stored and deflate (OCF allows no others).

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Cursor, Read, Write};

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, DateTime, ZipArchive, ZipWriter};

use super::diag::{self, Diagnostics};
use super::path::{is_safe_entry, resolve};
use super::xml::{self, CONTAINER, XMLENC};

pub const MIMETYPE: &str = "application/epub+zip";
pub const CONTAINER_XML: &str = "META-INF/container.xml";
pub const ENCRYPTION_XML: &str = "META-INF/encryption.xml";
const PACKAGE_MEDIA_TYPE: &str = "application/oebps-package+xml";
/// Font obfuscation algorithms (IDPF and Adobe): not DRM, and Atelier never
/// uses publisher fonts anyway.
const FONT_OBFUSCATION: [&str; 2] = [
    "http://www.idpf.org/2008/embedding",
    "http://ns.adobe.com/pdf/enc#RC",
];

/// Bounds on what one EPUB may inflate to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub max_entries: usize,
    pub max_entry_bytes: u64,
    pub max_total_bytes: u64,
    /// Inflated ÷ compressed, checked for entries over 1 MiB.
    pub max_ratio: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_entries: 10_000,
            max_entry_bytes: 64 << 20,
            max_total_bytes: 512 << 20,
            max_ratio: 200,
        }
    }
}

/// The container's files, inflated, by container path.
#[derive(Debug, Clone, Default)]
pub struct Container {
    files: BTreeMap<String, Vec<u8>>,
    /// Container path of the package document.
    pub rootfile: String,
    /// Resources under font obfuscation (left untouched).
    pub obfuscated: BTreeSet<String>,
}

impl Container {
    #[must_use]
    pub fn get(&self, path: &str) -> Option<&[u8]> {
        self.files.get(path).map(Vec::as_slice)
    }

    #[must_use]
    pub fn contains(&self, path: &str) -> bool {
        self.files.contains_key(path)
    }

    pub fn paths(&self) -> impl Iterator<Item = &str> {
        self.files.keys().map(String::as_str)
    }

    #[must_use]
    pub fn total_bytes(&self) -> usize {
        self.files.values().map(Vec::len).sum()
    }
}

fn check_mimetype(zip: &mut ZipArchive<Cursor<&[u8]>>, d: &mut Diagnostics) {
    let Ok(mut first) = zip.by_index_raw(0) else {
        d.push(
            diag::OCF_MIMETYPE_FIRST,
            "mimetype",
            None,
            "the archive is empty",
        );
        return;
    };
    if first.name() != "mimetype" {
        d.push(
            diag::OCF_MIMETYPE_FIRST,
            "mimetype",
            None,
            format!("first entry is `{}`", first.name()),
        );
        return;
    }
    if first.compression() != CompressionMethod::Stored {
        d.push(
            diag::OCF_MIMETYPE_STORED,
            "mimetype",
            None,
            "stored with compression",
        );
        return;
    }
    let mut content = Vec::new();
    if first.by_ref().take(64).read_to_end(&mut content).is_err() || content != MIMETYPE.as_bytes()
    {
        d.push(
            diag::OCF_MIMETYPE_VALUE,
            "mimetype",
            None,
            format!("contains {:?}", String::from_utf8_lossy(&content)),
        );
    }
}

/// Inflates every entry within `limits`. `None` when the archive cannot be
/// used at all (the reason is in `d`).
pub fn read(bytes: &[u8], limits: &Limits, d: &mut Diagnostics) -> Option<Container> {
    let mut zip = match ZipArchive::new(Cursor::new(bytes)) {
        Ok(z) => z,
        Err(e) => {
            d.push(diag::OCF_ZIP, "", None, e.to_string());
            return None;
        }
    };
    if zip.len() > limits.max_entries {
        d.push(
            diag::OCF_LIMITS,
            "",
            None,
            format!("{} entries (limit {})", zip.len(), limits.max_entries),
        );
        return None;
    }
    check_mimetype(&mut zip, d);
    let files = inflate_entries(&mut zip, limits, d)?;
    let mut container = Container {
        files,
        ..Container::default()
    };
    container.rootfile = rootfile(&container, d)?;
    check_encryption(&mut container, d);
    tracing::debug!(
        entries = container.files.len(),
        bytes = container.total_bytes(),
        rootfile = %container.rootfile,
        "container read"
    );
    Some(container)
}

/// Every safe, supported entry, inflated within `limits`.
fn inflate_entries(
    zip: &mut ZipArchive<Cursor<&[u8]>>,
    limits: &Limits,
    d: &mut Diagnostics,
) -> Option<BTreeMap<String, Vec<u8>>> {
    let mut files = BTreeMap::new();
    let mut total: u64 = 0;
    for i in 0..zip.len() {
        let (name, size) = {
            let raw = match zip.by_index_raw(i) {
                Ok(f) => f,
                Err(e) => {
                    d.push(diag::OCF_ZIP, "", None, format!("entry {i}: {e}"));
                    return None;
                }
            };
            let name = raw.name().to_owned();
            if raw.is_dir() {
                continue;
            }
            if !is_safe_entry(&name) {
                d.push(diag::OCF_PATH, &name, None, "unsafe entry name, skipped");
                continue;
            }
            if raw.encrypted() {
                d.push(diag::OCF_DRM, &name, None, "ZIP-level encryption");
                continue;
            }
            if !matches!(
                raw.compression(),
                CompressionMethod::Stored | CompressionMethod::Deflated
            ) {
                d.push(
                    diag::OCF_COMPRESSION,
                    &name,
                    None,
                    format!("{:?}", raw.compression()),
                );
                continue;
            }
            let size = raw.size();
            let ratio_exceeded = size > 1 << 20
                && raw.compressed_size() > 0
                && size / raw.compressed_size() > limits.max_ratio;
            total = total.saturating_add(size);
            if size > limits.max_entry_bytes || total > limits.max_total_bytes || ratio_exceeded {
                d.push(
                    diag::OCF_LIMITS,
                    &name,
                    None,
                    format!(
                        "{size} bytes from {} compressed (archive total {total})",
                        raw.compressed_size()
                    ),
                );
                return None;
            }
            (name, size)
        };
        if files.contains_key(&name) {
            d.push(diag::OCF_DUPLICATE, &name, None, "second copy ignored");
            continue;
        }
        let mut data = Vec::with_capacity(usize::try_from(size).unwrap_or(0));
        let read = zip.by_index(i).map_err(|e| e.to_string()).and_then(|f| {
            f.take(limits.max_entry_bytes + 1)
                .read_to_end(&mut data)
                .map_err(|e| e.to_string())
        });
        match read {
            Ok(n) if n as u64 == size => {}
            Ok(n) => {
                d.push(
                    diag::OCF_LIMITS,
                    &name,
                    None,
                    format!("inflated to {n} bytes, header says {size}"),
                );
                return None;
            }
            Err(e) => {
                d.push(diag::OCF_ZIP, &name, None, e);
                return None;
            }
        }
        files.insert(name, data);
    }
    Some(files)
}

fn rootfile(c: &Container, d: &mut Diagnostics) -> Option<String> {
    let fail = |d: &mut Diagnostics, line: Option<u32>, msg: String| {
        d.push(diag::OCF_CONTAINER, CONTAINER_XML, line, msg);
        None
    };
    let Some(bytes) = c.get(CONTAINER_XML) else {
        return fail(d, None, "missing".into());
    };
    let Ok(text) = core::str::from_utf8(bytes) else {
        return fail(d, None, "not UTF-8".into());
    };
    let doc = match xml::parse(text) {
        Ok(doc) => doc,
        Err(e) => return fail(d, Some(e.line), e.message),
    };
    let rf = doc
        .root
        .find(&|e| e.is(CONTAINER, "rootfile") && e.attr("media-type") == Some(PACKAGE_MEDIA_TYPE));
    let Some(path) = rf.and_then(|e| e.attr("full-path")) else {
        return fail(d, None, "no rootfile for an OPF package".into());
    };
    if !c.contains(path) {
        return fail(
            d,
            rf.map(|e| e.line),
            format!("rootfile `{path}` is not in the archive"),
        );
    }
    Some(path.to_owned())
}

fn check_encryption(c: &mut Container, d: &mut Diagnostics) {
    let Some(text) = c
        .get(ENCRYPTION_XML)
        .and_then(|b| core::str::from_utf8(b).ok())
    else {
        return;
    };
    let doc = match xml::parse(text) {
        Ok(doc) => doc,
        Err(e) => {
            d.push(
                diag::OCF_DRM,
                ENCRYPTION_XML,
                Some(e.line),
                "unreadable encryption.xml",
            );
            return;
        }
    };
    let mut obfuscated = BTreeSet::new();
    for data in doc
        .root
        .elements()
        .filter(|e| e.is(XMLENC, "EncryptedData"))
    {
        let algorithm = data
            .find(&|e| e.is(XMLENC, "EncryptionMethod"))
            .and_then(|e| e.attr("Algorithm"))
            .unwrap_or("");
        let target = data
            .find(&|e| e.is(XMLENC, "CipherReference"))
            .and_then(|e| e.attr("URI"))
            .and_then(|uri| resolve("", "", uri))
            .map(|t| t.path)
            .unwrap_or_default();
        if FONT_OBFUSCATION.contains(&algorithm) {
            obfuscated.insert(target);
        } else {
            d.push(
                diag::OCF_DRM,
                &target,
                Some(data.line),
                format!("encrypted with {algorithm}"),
            );
        }
    }
    c.obfuscated = obfuscated;
}

/// Writes a ZIP with exactly these entries, in this order: `stored` entries
/// uncompressed, the rest deflated. Timestamps are fixed, so the output is
/// reproducible. For OCF containers use [`pack`]; this is also how tests
/// build broken archives.
pub fn write_zip(entries: &[(&str, &[u8], bool)]) -> zip::result::ZipResult<Vec<u8>> {
    let mut w = ZipWriter::new(Cursor::new(Vec::new()));
    for &(name, data, stored) in entries {
        let opts = SimpleFileOptions::default()
            .compression_method(if stored {
                CompressionMethod::Stored
            } else {
                CompressionMethod::Deflated
            })
            .last_modified_time(DateTime::default())
            .unix_permissions(0o644);
        w.start_file(name, opts)?;
        w.write_all(data)?;
    }
    Ok(w.finish()?.into_inner())
}

/// An OCF container: `mimetype` first and stored, then `files` deflated in
/// path order.
pub fn pack(files: &BTreeMap<String, Vec<u8>>) -> zip::result::ZipResult<Vec<u8>> {
    let mut entries: Vec<(&str, &[u8], bool)> = vec![("mimetype", MIMETYPE.as_bytes(), true)];
    entries.extend(
        files
            .iter()
            .filter(|(k, _)| k.as_str() != "mimetype")
            .map(|(k, v)| (k.as_str(), v.as_slice(), false)),
    );
    write_zip(&entries)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    const CONTAINER_OK: &[u8] = br#"<?xml version="1.0"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles>
</container>"#;

    fn base() -> Vec<(&'static str, &'static [u8], bool)> {
        vec![
            ("mimetype", MIMETYPE.as_bytes(), true),
            (CONTAINER_XML, CONTAINER_OK, false),
            ("OEBPS/content.opf", b"<package/>", false),
        ]
    }

    fn read_entries(entries: &[(&str, &[u8], bool)]) -> (Option<Container>, Diagnostics) {
        let bytes = write_zip(entries).unwrap();
        let mut d = Diagnostics::default();
        let c = read(&bytes, &Limits::default(), &mut d);
        (c, d)
    }

    #[test]
    fn reads_a_minimal_container() {
        let (c, d) = read_entries(&base());
        let c = c.unwrap();
        assert!(d.is_empty(), "{d:?}");
        assert_eq!(c.rootfile, "OEBPS/content.opf");
        assert_eq!(c.get("OEBPS/content.opf"), Some(&b"<package/>"[..]));
    }

    #[test]
    fn mimetype_rules() {
        let mut e = base();
        e.swap(0, 1);
        assert!(read_entries(&e).1.has(diag::OCF_MIMETYPE_FIRST));
        let mut e = base();
        e[0] = ("mimetype", b"application/epub+zip\n", true);
        assert!(read_entries(&e).1.has(diag::OCF_MIMETYPE_VALUE));
        let mut e = base();
        e[0].2 = false;
        assert!(read_entries(&e).1.has(diag::OCF_MIMETYPE_STORED));
    }

    #[test]
    fn container_xml_rules() {
        let e: Vec<_> = base()
            .into_iter()
            .filter(|x| x.0 != CONTAINER_XML)
            .collect();
        let (c, d) = read_entries(&e);
        assert!(c.is_none() && d.has(diag::OCF_CONTAINER));
        let e: Vec<_> = base()
            .into_iter()
            .filter(|x| x.0 != "OEBPS/content.opf")
            .collect();
        assert!(read_entries(&e).1.has(diag::OCF_CONTAINER));
    }

    #[test]
    fn unsafe_names_are_skipped() {
        let mut e = base();
        e.push(("../evil.txt", b"x", false));
        let (c, d) = read_entries(&e);
        assert!(d.has(diag::OCF_PATH));
        assert!(!c.unwrap().paths().any(|p| p.contains("evil")));
    }

    #[test]
    fn zip_bombs_are_refused() {
        let zeros = vec![0u8; 4 << 20]; // deflates ~1000:1
        let mut e = base();
        e.push(("OEBPS/bomb.bin", &zeros, false));
        let (c, d) = read_entries(&e);
        assert!(c.is_none() && d.has(diag::OCF_LIMITS));
        let small = Limits {
            max_total_bytes: 100,
            ..Limits::default()
        };
        let bytes = write_zip(&base()).unwrap();
        let mut d = Diagnostics::default();
        assert!(
            read(
                &bytes,
                &Limits {
                    max_total_bytes: 1,
                    ..small
                },
                &mut d
            )
            .is_none()
        );
    }

    #[test]
    fn drm_is_refused_font_obfuscation_is_not() {
        let enc = |alg: &str| {
            format!(
                r#"<encryption xmlns="urn:oasis:names:tc:opendocument:xmlns:container" xmlns:enc="http://www.w3.org/2001/04/xmlenc#">
<enc:EncryptedData><enc:EncryptionMethod Algorithm="{alg}"/>
<enc:CipherData><enc:CipherReference URI="OEBPS/x.bin"/></enc:CipherData></enc:EncryptedData></encryption>"#
            )
        };
        for (alg, drm) in [
            ("http://www.idpf.org/2008/embedding", false),
            ("http://www.w3.org/2001/04/xmlenc#aes128-cbc", true),
        ] {
            let xml = enc(alg);
            let mut e = base();
            e.push((ENCRYPTION_XML, xml.as_bytes(), false));
            let (c, d) = read_entries(&e);
            assert_eq!(d.has(diag::OCF_DRM), drm, "{alg}");
            assert_eq!(c.unwrap().obfuscated.contains("OEBPS/x.bin"), !drm);
        }
    }

    #[test]
    fn not_a_zip() {
        let mut d = Diagnostics::default();
        assert!(read(b"PK\x03\x04 nope", &Limits::default(), &mut d).is_none());
        assert!(d.has(diag::OCF_ZIP));
    }

    #[test]
    fn pack_puts_mimetype_first_and_stored() {
        let mut files = BTreeMap::new();
        files.insert(CONTAINER_XML.to_owned(), CONTAINER_OK.to_vec());
        files.insert("OEBPS/content.opf".to_owned(), b"<package/>".to_vec());
        let bytes = pack(&files).unwrap();
        let mut d = Diagnostics::default();
        assert!(read(&bytes, &Limits::default(), &mut d).is_some());
        assert!(d.is_empty(), "{d:?}");
        assert_eq!(pack(&files).unwrap(), bytes, "reproducible");
    }
}
