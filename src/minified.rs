//! Minified JavaScript and CSS, which ast-index leaves out of every walk.
//!
//! A minified bundle is build output, not source: it floods search with
//! one-letter names and bundler internals, and a stylesheet squeezed onto one
//! line gives each of its thousands of selectors the whole file as a
//! signature. Indexing, `update`, the grep-based commands and the per-file
//! commands all ask [`skip`] before reading a file.

use std::fs::File;
use std::io::Read;
use std::path::Path;

/// Bytes from the start of a file that the content check looks at.
pub const SAMPLE_BYTES: usize = 64 * 1024;

/// Average line length in the sample, in bytes, from which a file may count
/// as minified. A minifier's output runs to thousands; formatted source stays
/// in the tens even with a few embedded images or SVG paths.
const MINIFIED_AVERAGE_LINE_BYTES: usize = 1000;

/// Of that average, the bytes outside string literals a minified file has at
/// least. Source whose long lines are single strings (HTML or legal-text
/// templates, data URIs) keeps a few dozen, code squeezed onto a line far more.
const MINIFIED_AVERAGE_CODE_BYTES: usize = 100;

/// Set to `0` to index and search minified files like any other source.
pub const SKIP_ENV: &str = "AST_INDEX_SKIP_MINIFIED";

/// Whether ast-index should leave `path` out: its name marks it as minified,
/// or it is a file type minifiers emit and the start of its content reads as
/// minified. `head` is the start of the file when the caller already has it;
/// otherwise the sample is read from disk.
pub fn skip(path: &Path, head: Option<&[u8]>) -> bool {
    enabled() && is_minified(path, head)
}

/// The part of [`skip`] that needs no file access: whether the name alone
/// marks `path` as minified.
pub fn skip_by_name(path: &Path) -> bool {
    enabled() && name_is_minified(path)
}

/// Whether [`skip`] reads the start of `path` when no `head` is given. A
/// caller about to read the whole file anyway reads it first and passes it,
/// sparing a second open.
pub fn judged_by_content(path: &Path) -> bool {
    enabled() && !name_is_minified(path) && is_minifiable(path)
}

/// Whether the minified filter is on (the default).
pub fn enabled() -> bool {
    match std::env::var(SKIP_ENV) {
        Ok(value) => !matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "no" | "off"
        ),
        Err(_) => true,
    }
}

fn is_minified(path: &Path, head: Option<&[u8]>) -> bool {
    if name_is_minified(path) {
        return true;
    }
    if !is_minifiable(path) {
        return false;
    }
    match head {
        Some(head) => sample_is_minified(&head[..head.len().min(SAMPLE_BYTES)]),
        None => read_sample(path).is_ok_and(|sample| sample_is_minified(&sample)),
    }
}

/// Only extensions a minifier writes are judged by content. A long line in
/// TypeScript, JSX or SCSS is a hand-written string or a compiler's `.d.ts`
/// union type, both worth indexing.
fn is_minifiable(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(is_minifiable_extension)
}

fn is_minifiable_extension(ext: &str) -> bool {
    ["js", "mjs", "cjs", "css"]
        .iter()
        .any(|known| known.eq_ignore_ascii_case(ext))
}

/// `app.min.js`, `vendor-min.css` and the like.
fn name_is_minified(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let Some((stem, ext)) = name.rsplit_once('.') else {
        return false;
    };
    if !is_minifiable_extension(ext) {
        return false;
    }
    let stem = stem.as_bytes();
    let Some(split) = stem.len().checked_sub(4).filter(|&split| split > 0) else {
        return false;
    };
    let (separator, marker) = stem[split..].split_at(1);
    matches!(separator, b"." | b"-") && marker.eq_ignore_ascii_case(b"min")
}

fn read_sample(path: &Path) -> std::io::Result<Vec<u8>> {
    // Room for the whole sample up front: grown from empty, the buffer would
    // cost a read call per doubling.
    let mut sample = Vec::with_capacity(SAMPLE_BYTES);
    File::open(path)?
        .take(SAMPLE_BYTES as u64)
        .read_to_end(&mut sample)?;
    Ok(sample)
}

fn sample_is_minified(sample: &[u8]) -> bool {
    let Some(&last) = sample.last() else {
        return false;
    };
    let newlines = sample.iter().filter(|&&byte| byte == b'\n').count();
    let lines = newlines + usize::from(last != b'\n');
    sample.len() / lines >= MINIFIED_AVERAGE_LINE_BYTES
        && bytes_outside_strings(sample) / lines >= MINIFIED_AVERAGE_CODE_BYTES
}

/// A rough lexer, good enough to tell code from string contents: quotes and
/// backslash escapes are tracked, `'` and `"` strings end at a line break,
/// template literals run on.
fn bytes_outside_strings(sample: &[u8]) -> usize {
    let mut code = 0;
    let mut quote = None;
    let mut escaped = false;
    for &byte in sample {
        if byte == b'\n' {
            escaped = false;
            if quote != Some(b'`') {
                quote = None;
            }
            continue;
        }
        match quote {
            Some(_) if escaped => escaped = false,
            Some(_) if byte == b'\\' => escaped = true,
            Some(open) if byte == open => quote = None,
            Some(_) => {}
            None if matches!(byte, b'"' | b'\'' | b'`') => quote = Some(byte),
            None => code += 1,
        }
    }
    code
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minified(name: &str, content: &str) -> bool {
        is_minified(Path::new(name), Some(content.as_bytes()))
    }

    fn one_line_bundle() -> String {
        "var a=function(b,c){return b+c},d=[1,2,3].map(a);".repeat(60)
    }

    #[test]
    fn min_suffix_marks_web_files_whatever_the_content() {
        for name in [
            "app.min.js",
            "lib/app.bundle.min.js",
            "vendor-min.js",
            "theme.min.css",
            "chunk.min.mjs",
            "chunk.min.cjs",
            "Legacy.MIN.JS",
        ] {
            assert!(minified(name, "let a = 1;\n"), "{name}");
        }
    }

    #[test]
    fn min_in_other_positions_is_a_plain_name() {
        for name in [
            "min.js",
            ".min.js",
            "admin.js",
            "find_min.js",
            "minimal.css",
            "app.min.ts",
            "styles.min.scss",
            "heap-min.c",
            "app.min.js.map",
        ] {
            assert!(!minified(name, "let a = 1;\n"), "{name}");
        }
    }

    #[test]
    fn a_bundle_on_one_line_is_minified() {
        assert!(minified("public/vendor.js", &one_line_bundle()));
        assert!(minified("public/vendor.mjs", &one_line_bundle()));
        assert!(minified("public/vendor.cjs", &one_line_bundle()));
    }

    #[test]
    fn a_license_header_does_not_hide_the_bundle() {
        let header = "/*!\n * Widget v1.2.3\n * Copyright Example\n * MIT License\n */\n";
        let content = format!("{header}{}", one_line_bundle().repeat(20));
        assert!(minified("public/widget.js", &content));
    }

    #[test]
    fn a_minified_stylesheet_is_minified() {
        let sheet = ".a{color:red;margin:0}.b>.c{padding:1px 2px}".repeat(40);
        assert!(minified("public/site.css", &sheet));
    }

    #[test]
    fn source_with_a_few_long_lines_is_not_minified() {
        let mut source = String::from("export function Logo() {\n");
        source.push_str(&format!("  const d = \"M0 0{}\";\n", " L1 2".repeat(800)));
        source.push_str(&format!(
            "  const src = \"data:image/png;base64,{}\";\n",
            "iVBORw0KGgo".repeat(250)
        ));
        for i in 0..40 {
            source.push_str(&format!("  const part{i} = d.slice({i});\n"));
        }
        source.push_str("  return <svg><path d={d} /></svg>;\n}\n");
        assert!(!minified("src/Logo.js", &source));
    }

    #[test]
    fn long_string_templates_are_not_minified() {
        let paragraph = "<p>The subject of personal data agrees to processing.</p>".repeat(90);
        let indented = format!(
            "export default function useTemplate() {{\n  const AGREEMENT = '{paragraph}'\n  const EMAIL = \"{paragraph}\"\n  const BODY = `{paragraph}`\n  return {{ AGREEMENT, EMAIL, BODY }}\n}}\n"
        );
        assert!(!minified("src/templates.js", &indented));

        let top_level = format!("export const AGREEMENT = '{paragraph}';\n");
        assert!(!minified("src/agreement.js", &top_level));

        let escaped = format!("export const QUOTE = 'it\\'s {paragraph}';\n");
        assert!(!minified("src/quote.js", &escaped));
    }

    #[test]
    fn a_minified_stylesheet_opening_with_a_font_is_minified() {
        let font = "d09GMgABAAAAA".repeat(2000);
        let sheet = format!(
            "@font-face{{font-family:Icons;src:url(\"data:font/woff2;base64,{font}\")}}{}",
            ".i-a:before{content:\"\\e900\"}.i-b{display:inline-block;width:1em}".repeat(40)
        );
        assert!(minified("public/icons.css", &sheet));
    }

    #[test]
    fn a_short_one_liner_is_not_minified() {
        assert!(!minified(
            "babel.config.js",
            "module.exports = { presets: ['@babel/preset-env'] };\n"
        ));
        assert!(!minified("empty.js", ""));
    }

    #[test]
    fn only_minifier_output_types_are_judged_by_content() {
        let bundle = one_line_bundle();
        for name in [
            "src/table.ts",
            "src/Icon.tsx",
            "src/Icon.jsx",
            "types/nodes.d.ts",
            "styles/theme.scss",
            "db/seeds.rb",
        ] {
            assert!(!minified(name, &bundle), "{name}");
        }
    }

    #[test]
    fn code_past_the_sample_is_not_judged() {
        let mut content = String::new();
        for i in 0..(SAMPLE_BYTES / 20) {
            content.push_str(&format!("const v{i:07} = 1;\n"));
        }
        content.push_str(&one_line_bundle().repeat(100));
        assert!(content.len() > 2 * SAMPLE_BYTES);
        assert!(!minified("src/generated.js", &content));
    }

    #[test]
    fn the_sample_is_read_from_disk_without_a_head() {
        let dir = tempfile::TempDir::new().unwrap();
        let bundle = dir.path().join("vendor.js");
        std::fs::write(&bundle, one_line_bundle()).unwrap();
        let source = dir.path().join("app.js");
        std::fs::write(&source, "export const a = 1;\n").unwrap();

        assert!(is_minified(&bundle, None));
        assert!(!is_minified(&source, None));
        assert!(!is_minified(&dir.path().join("missing.js"), None));
    }
}
