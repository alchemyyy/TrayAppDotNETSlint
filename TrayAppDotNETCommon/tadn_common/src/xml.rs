//! XML settings persistence (port of `Serialization/*`: the generated element-per-property
//! serializer rules, `TrayXmlFileStore`, `DataFileLoader`, `AtomicFile`).
//!
//! File rules (identical to the .NET apps so files are interchangeable):
//! - UTF-8 with BOM, `<?xml version="1.0" encoding="utf-8"?>`, 2-space indent, CRLF.
//! - One element per field; enums by member name; unknown elements skipped; an unparsable value
//!   keeps the default; empty string element reads back as `""`.
//! - Missing file → defaults (file created on first save). Invalid XML or wrong root → the file is
//!   renamed `<name>.corrupt-<UTC yyyyMMddHHmmss>` and defaults are used. Locked after retries →
//!   defaults and the store becomes read-only for the session.
//! - Every write goes to a temp file in the same folder, then replaces the target.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::fs::OpenOptions;
use std::io::{self, Read, Write as _};
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, LazyLock, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime};

use crate::settings::XmlEnum;

/// `TimeConstants.SettingsSaveDebounceMs`: quiet period of an implicit save.
pub const SETTINGS_SAVE_DEBOUNCE_MS: u64 = 400;
/// `TimeConstants.SettingsFileReplaceRetryCount`: attempts for locked reads, replaces, temp
/// deletes and writer rounds.
pub const SETTINGS_FILE_REPLACE_RETRY_COUNT: u32 = 6;
/// `TimeConstants.SettingsFileReplaceRetryDelayMs`.
pub const SETTINGS_FILE_REPLACE_RETRY_DELAY_MS: u64 = 50;

const XML_DECLARATION: &str = "<?xml version=\"1.0\" encoding=\"utf-8\"?>";
const NEW_LINE: &str = "\r\n";
const INDENT: &str = "  ";

/// A document that cannot be read: XML syntax error, wrong root, or content the generated C#
/// reader rejects (an element with child elements where a scalar value is expected).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct XmlError(pub String);

impl std::fmt::Display for XmlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for XmlError {}

/// Parsed element tree (owned DOM). Consumers use the accessor methods.
///
/// `text` holds the element's own text content the way `XmlReader.ReadElementContentAsString`
/// returns it with `IgnoreWhitespace`: text and CDATA nodes concatenated, whitespace-only nodes
/// dropped, comments and processing instructions ignored.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct XmlElement {
    pub name: String,
    pub attributes: Vec<(String, String)>,
    pub children: Vec<XmlElement>,
    pub text: String,
}

impl XmlElement {
    /// First child element called `name`.
    pub fn child(&self, name: &str) -> Option<&XmlElement> {
        self.children.iter().find(|c| c.name == name)
    }

    /// Last child element called `name` (the one the generated reader keeps when a scalar element
    /// repeats).
    pub fn last_child(&self, name: &str) -> Option<&XmlElement> {
        self.children.iter().rev().find(|c| c.name == name)
    }

    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a XmlElement> + 'a {
        self.children.iter().filter(move |c| c.name == name)
    }

    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attributes.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
    }

    /// Scalar content (`ReadElementContentAsString`). An element with child elements is an error:
    /// the C# reader throws, which makes the whole file corrupt.
    pub fn content(&self) -> Result<&str, XmlError> {
        if self.children.is_empty() {
            Ok(&self.text)
        } else {
            Err(XmlError(format!("element <{}> has child elements where a value is expected", self.name)))
        }
    }

    /// `bool` property: an unparsable value keeps `target`.
    pub fn read_bool(&self, target: &mut bool) -> Result<(), XmlError> {
        if let Some(value) = parse_bool(self.content()?) {
            *target = value;
        }
        Ok(())
    }

    /// `int` property: an unparsable value keeps `target`.
    pub fn read_i32(&self, target: &mut i32) -> Result<(), XmlError> {
        if let Some(value) = parse_i32(self.content()?) {
            *target = value;
        }
        Ok(())
    }

    /// `double` property: an unparsable value keeps `target`.
    pub fn read_f64(&self, target: &mut f64) -> Result<(), XmlError> {
        if let Some(value) = parse_f64(self.content()?) {
            *target = value;
        }
        Ok(())
    }

    /// Enum property: an unknown name keeps `target` (`PE`).
    pub fn read_enum<E: XmlEnum>(&self, target: &mut E) -> Result<(), XmlError> {
        if let Some(value) = E::from_xml_name(self.content()?) {
            *target = value;
        }
        Ok(())
    }

    /// `string` property: the element text (`""` for an empty element).
    pub fn read_string(&self, target: &mut String) -> Result<(), XmlError> {
        self.content()?.clone_into(target);
        Ok(())
    }

    /// `string?` property mapped to Option: an empty element is None.
    pub fn read_opt_string(&self, target: &mut Option<String>) -> Result<(), XmlError> {
        let text = self.content()?;
        *target = (!text.is_empty()).then(|| text.to_owned());
        Ok(())
    }

    /// Parses `bytes` (UTF-8 with or without BOM, or UTF-16 with BOM) with DTDs prohibited and
    /// returns the root element.
    pub fn parse(bytes: &[u8]) -> Result<XmlElement, XmlError> {
        let text = decode(bytes)?;
        let options = roxmltree::ParsingOptions { allow_dtd: false, ..roxmltree::ParsingOptions::default() };
        let document = roxmltree::Document::parse_with_options(&text, options)
            .map_err(|error| XmlError(format!("XML parse error: {error}")))?;
        Ok(Self::from_node(document.root_element()))
    }

    fn from_node(node: roxmltree::Node<'_, '_>) -> XmlElement {
        let mut element = XmlElement {
            name: node.tag_name().name().to_owned(),
            attributes: node.attributes().map(|a| (a.name().to_owned(), a.value().to_owned())).collect(),
            children: Vec::new(),
            text: String::new(),
        };
        for child in node.children() {
            if child.is_element() {
                element.children.push(Self::from_node(child));
            } else if child.is_text() {
                let text = child.text().unwrap_or_default();
                if !text.chars().all(is_xml_whitespace) {
                    element.text.push_str(text);
                }
            }
        }
        element
    }
}

fn decode(bytes: &[u8]) -> Result<String, XmlError> {
    let utf16 = |data: &[u8], big_endian: bool| {
        let units: Vec<u16> = data
            .chunks_exact(2)
            .map(|pair| {
                if big_endian { u16::from_be_bytes([pair[0], pair[1]]) } else { u16::from_le_bytes([pair[0], pair[1]]) }
            })
            .collect();
        String::from_utf16(&units).map_err(|_| XmlError("invalid UTF-16 text".to_owned()))
    };
    match bytes {
        [0xEF, 0xBB, 0xBF, rest @ ..] => {
            String::from_utf8(rest.to_vec()).map_err(|_| XmlError("invalid UTF-8 text".to_owned()))
        }
        [0xFF, 0xFE, rest @ ..] => utf16(rest, false),
        [0xFE, 0xFF, rest @ ..] => utf16(rest, true),
        _ => String::from_utf8(bytes.to_vec()).map_err(|_| XmlError("invalid UTF-8 text".to_owned())),
    }
}

fn is_xml_whitespace(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r')
}

// ---------------------------------------------------------------------------------------------
// Value conversions (XmlConvert)
// ---------------------------------------------------------------------------------------------

/// `XmlConvert.ToBoolean`: `true`/`1`, `false`/`0` after trimming XML whitespace.
pub fn parse_bool(text: &str) -> Option<bool> {
    match text.trim_matches(is_xml_whitespace) {
        "true" | "1" => Some(true),
        "false" | "0" => Some(false),
        _ => None,
    }
}

/// `XmlConvert.ToString(bool)`.
pub fn format_bool(value: bool) -> &'static str {
    if value { "true" } else { "false" }
}

fn trim_number(text: &str) -> &str {
    text.trim_matches(|c: char| matches!(c, ' ' | '\t' | '\n' | '\u{B}' | '\u{C}' | '\r'))
}

/// `XmlConvert.ToInt32`: optional sign, decimal digits, surrounding whitespace allowed.
pub fn parse_i32(text: &str) -> Option<i32> {
    let text = trim_number(text);
    if text.is_empty() || !text.trim_start_matches(['+', '-']).bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// `XmlConvert.ToUInt32` (`-0` is zero like `UInt32.Parse`).
pub fn parse_u32(text: &str) -> Option<u32> {
    let text = trim_number(text);
    if let Some(digits) = text.strip_prefix('-') {
        return (!digits.is_empty() && digits.bytes().all(|b| b == b'0')).then_some(0);
    }
    if text.is_empty() || !text.trim_start_matches('+').bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// `XmlConvert.ToDouble`: `INF`/`-INF`, invariant decimal or exponent notation, `NaN`.
pub fn parse_f64(text: &str) -> Option<f64> {
    let text = text.trim_matches(is_xml_whitespace);
    match text {
        "INF" => return Some(f64::INFINITY),
        "-INF" => return Some(f64::NEG_INFINITY),
        _ => {}
    }
    let unsigned = text.strip_prefix(['+', '-']).unwrap_or(text);
    let negative = text.starts_with('-');
    if unsigned.eq_ignore_ascii_case("NaN") {
        return Some(f64::NAN);
    }
    if unsigned.eq_ignore_ascii_case("Infinity") {
        return Some(if negative { f64::NEG_INFINITY } else { f64::INFINITY });
    }
    let (mantissa, exponent) = match unsigned.find(['e', 'E']) {
        Some(index) => (&unsigned[..index], Some(&unsigned[index + 1..])),
        None => (unsigned, None),
    };
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let digits_ok = whole.bytes().all(|b| b.is_ascii_digit())
        && fraction.bytes().all(|b| b.is_ascii_digit())
        && !(whole.is_empty() && fraction.is_empty());
    let exponent_ok = exponent.is_none_or(|e| {
        let e = e.strip_prefix(['+', '-']).unwrap_or(e);
        !e.is_empty() && e.bytes().all(|b| b.is_ascii_digit())
    });
    if !digits_ok || !exponent_ok {
        return None;
    }
    let sign = if negative { "-" } else { "" };
    let exponent = exponent.map_or(String::new(), |e| format!("e{e}"));
    format!("{sign}0{whole}.{fraction}0{exponent}").parse().ok()
}

/// `XmlConvert.ToString(double)`: `INF`, `-INF`, `NaN`, `-0`, otherwise the shortest round-trip
/// text in .NET's `"R"` layout (scientific `1E+15` style beyond 15 significant positions or below
/// `1E-04`).
pub fn format_f64(value: f64) -> String {
    if value.is_nan() {
        return "NaN".to_owned();
    }
    if value.is_infinite() {
        return if value > 0.0 { "INF" } else { "-INF" }.to_owned();
    }
    if value == 0.0 {
        return if value.is_sign_negative() { "-0" } else { "0" }.to_owned();
    }
    let scientific = format!("{:e}", value.abs());
    let (mantissa, exponent) = scientific.split_once('e').unwrap_or((&scientific, "0"));
    let exponent: i32 = exponent.parse().unwrap_or(0);
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let digit_count = digits.len() as i32;
    let scale = exponent + 1;
    let max_digits = digit_count.max(15);
    let mut out = String::with_capacity(digits.len() + 8);
    if value < 0.0 {
        out.push('-');
    }
    if scale > max_digits || scale < -3 {
        out.push_str(&digits[..1]);
        if digits.len() > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        let _ = write!(out, "E{}{:02}", if exponent < 0 { '-' } else { '+' }, exponent.unsigned_abs());
    } else if scale <= 0 {
        out.push_str("0.");
        out.extend(std::iter::repeat_n('0', (-scale) as usize));
        out.push_str(&digits);
    } else if scale >= digit_count {
        out.push_str(&digits);
        out.extend(std::iter::repeat_n('0', (scale - digit_count) as usize));
    } else {
        out.push_str(&digits[..scale as usize]);
        out.push('.');
        out.push_str(&digits[scale as usize..]);
    }
    out
}

/// True for characters XML 1.0 can carry (`XmlConvert.IsXmlChar`; Rust strings hold no lone
/// surrogates, so every supplementary character is valid).
fn is_xml_char(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\r') || (c >= ' ' && c != '\u{FFFE}' && c != '\u{FFFF}')
}

/// Drops characters XML cannot carry (the generated writer's `F(string)`).
pub fn strip_invalid_xml_chars(value: &str) -> std::borrow::Cow<'_, str> {
    if value.chars().all(is_xml_char) {
        std::borrow::Cow::Borrowed(value)
    } else {
        std::borrow::Cow::Owned(value.chars().filter(|&c| is_xml_char(c)).collect())
    }
}

// ---------------------------------------------------------------------------------------------
// Writer
// ---------------------------------------------------------------------------------------------

struct OpenElement {
    name: String,
    /// The start tag is still open (`<Name attr="..."`): attributes can follow.
    start_tag_open: bool,
    has_children: bool,
    has_text: bool,
}

/// Serializer for one document with the output layout of .NET's `XmlWriter` (`Indent = true`,
/// `IndentChars = "  "`, `NewLineChars = "\r\n"`, `NewLineHandling.Replace`).
#[derive(Default)]
pub struct XmlWriter {
    pub(crate) out: String,
    pub(crate) depth: usize,
    stack: Vec<OpenElement>,
}

impl std::fmt::Debug for XmlWriter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("XmlWriter").field("out", &self.out).field("depth", &self.depth).finish()
    }
}

impl XmlWriter {
    /// A writer positioned after the BOM and the XML declaration.
    pub fn new() -> Self {
        let mut out = String::with_capacity(8 * 1024);
        out.push('\u{FEFF}');
        out.push_str(XML_DECLARATION);
        Self { out, depth: 0, stack: Vec::new() }
    }

    fn close_start_tag(&mut self) {
        if let Some(top) = self.stack.last_mut()
            && top.start_tag_open
        {
            self.out.push('>');
            top.start_tag_open = false;
        }
    }

    fn new_line_indent(&mut self) {
        self.out.push_str(NEW_LINE);
        for _ in 0..self.depth {
            self.out.push_str(INDENT);
        }
    }

    /// `WriteStartElement`.
    pub fn start_element(&mut self, name: &str) {
        self.close_start_tag();
        let mixed = self.stack.last().is_some_and(|parent| parent.has_text);
        if let Some(parent) = self.stack.last_mut() {
            parent.has_children = true;
        }
        if !mixed {
            self.new_line_indent();
        }
        self.out.push('<');
        self.out.push_str(name);
        self.stack.push(OpenElement {
            name: name.to_owned(),
            start_tag_open: true,
            has_children: false,
            has_text: false,
        });
        self.depth += 1;
    }

    /// `WriteAttributeString`; must follow [`Self::start_element`] before any content.
    pub fn attribute(&mut self, name: &str, value: &str) {
        debug_assert!(self.stack.last().is_some_and(|e| e.start_tag_open), "attribute after content");
        self.out.push(' ');
        self.out.push_str(name);
        self.out.push_str("=\"");
        escape_attribute(&mut self.out, &strip_invalid_xml_chars(value));
        self.out.push('"');
    }

    /// `WriteString`: element text content.
    pub fn text(&mut self, value: &str) {
        let value = strip_invalid_xml_chars(value);
        if value.is_empty() {
            return;
        }
        self.close_start_tag();
        if let Some(top) = self.stack.last_mut() {
            top.has_text = true;
        }
        escape_text(&mut self.out, &value);
    }

    /// `WriteEndElement`: `<X />` when empty, `</X>` after text, or on its own indented line after
    /// child elements.
    pub fn end_element(&mut self) {
        let Some(element) = self.stack.pop() else {
            debug_assert!(false, "end_element without start_element");
            return;
        };
        self.depth -= 1;
        if element.start_tag_open {
            self.out.push_str(" />");
            return;
        }
        if element.has_children && !element.has_text {
            self.new_line_indent();
        }
        self.out.push_str("</");
        self.out.push_str(&element.name);
        self.out.push('>');
    }

    /// `WriteElementString(name, value)`.
    pub fn element(&mut self, name: &str, value: &str) {
        self.start_element(name);
        self.text(value);
        self.end_element();
    }

    pub fn element_bool(&mut self, name: &str, value: bool) {
        self.element(name, format_bool(value));
    }

    pub fn element_i32(&mut self, name: &str, value: i32) {
        self.element(name, &value.to_string());
    }

    pub fn element_f64(&mut self, name: &str, value: f64) {
        self.element(name, &format_f64(value));
    }

    pub fn element_enum<E: XmlEnum + PartialEq>(&mut self, name: &str, value: E) {
        self.element(name, value.xml_name());
    }

    /// `string?` property: `None` writes an empty element.
    pub fn element_opt(&mut self, name: &str, value: Option<&str>) {
        self.element(name, value.unwrap_or_default());
    }

    /// The finished document (UTF-8 with BOM).
    pub fn finish(mut self) -> Vec<u8> {
        while !self.stack.is_empty() {
            self.end_element();
        }
        self.out.into_bytes()
    }
}

fn escape_text(out: &mut String, value: &str) {
    let mut chars = value.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push_str(NEW_LINE);
            }
            '\n' => out.push_str(NEW_LINE),
            _ => out.push(c),
        }
    }
}

fn escape_attribute(out: &mut String, value: &str) {
    for c in value.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\t' => out.push_str("&#x9;"),
            '\n' => out.push_str("&#xA;"),
            '\r' => out.push_str("&#xD;"),
            _ => out.push(c),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Documents
// ---------------------------------------------------------------------------------------------

/// A document type persisted by [`XmlFileStore`].
pub trait XmlDocument: Clone + Default + Send + 'static {
    /// Root element name; a mismatch on load counts as corrupt.
    const ROOT: &'static str;

    /// Reads the root element (infallible view: content the C# reader would reject yields the
    /// defaults). The store uses [`Self::try_read_xml`].
    fn read_xml(root: &XmlElement) -> Self;

    /// Writes the root element's attributes and children; the root start/end tags are written by
    /// [`serialize`].
    fn write_xml(&self, writer: &mut XmlWriter);

    /// Reads the root element; `Err` marks the file corrupt. Override when content can be
    /// rejected (scalar elements with child elements).
    fn try_read_xml(root: &XmlElement) -> Result<Self, XmlError> {
        Ok(Self::read_xml(root))
    }

    /// Load-time repair of a loaded or default document (the C# store's `normalize` callback).
    /// Returns true when persisted state changed; a loaded file is then saved once.
    fn normalize(&mut self) -> bool {
        false
    }

    /// Reads and normalizes; returns whether the loaded data needs a save. Override when reading
    /// itself drops persisted data (unknown hotkey actions).
    fn read_normalized(root: &XmlElement) -> Result<(Self, bool), XmlError> {
        let mut value = Self::try_read_xml(root)?;
        let changed = value.normalize();
        Ok((value, changed))
    }
}

/// Serializes a document: BOM, declaration, root element, CRLF layout; no trailing newline.
pub fn serialize<T: XmlDocument>(value: &T) -> Vec<u8> {
    let mut writer = XmlWriter::new();
    writer.start_element(T::ROOT);
    value.write_xml(&mut writer);
    writer.end_element();
    writer.finish()
}

/// Parses `bytes` as a `T` document: XML errors, a wrong root or rejected content are `Err`.
pub fn deserialize<T: XmlDocument>(bytes: &[u8]) -> Result<T, XmlError> {
    let root = parse_root::<T>(bytes)?;
    T::try_read_xml(&root)
}

fn parse_root<T: XmlDocument>(bytes: &[u8]) -> Result<XmlElement, XmlError> {
    let root = XmlElement::parse(bytes)?;
    if root.name != T::ROOT {
        return Err(XmlError(format!("Unexpected XML root <{}>, expected <{}>.", root.name, T::ROOT)));
    }
    Ok(root)
}

// ---------------------------------------------------------------------------------------------
// Loading (DataFileLoader)
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadState {
    Missing,
    Loaded,
    Corrupt { backup: Option<PathBuf> },
    Locked,
}

/// Raw outcome of [`try_load`]; nothing on disk is changed.
#[derive(Debug)]
pub enum LoadOutcome<T> {
    Missing,
    Loaded(T),
    Corrupt(XmlError),
    Locked(io::Error),
}

/// File or folder absent (`FileNotFound`/`DirectoryNotFound`).
fn is_not_found(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::NotFound
}

/// `DataFileLoader.TryLoad`: opens with share `Read|Delete`, retries an unreadable file
/// [`SETTINGS_FILE_REPLACE_RETRY_COUNT`] times [`SETTINGS_FILE_REPLACE_RETRY_DELAY_MS`] apart, then
/// reports it locked; a parse failure is corrupt.
pub fn try_load<V>(path: &Path, parse: impl Fn(&[u8]) -> Result<V, XmlError>) -> LoadOutcome<V> {
    use windows::Win32::Storage::FileSystem::{FILE_SHARE_DELETE, FILE_SHARE_READ};
    let mut attempt = 1;
    loop {
        if !path.exists() {
            return LoadOutcome::Missing;
        }
        let opened =
            OpenOptions::new().read(true).share_mode(FILE_SHARE_READ.0 | FILE_SHARE_DELETE.0).open(path).and_then(
                |mut file| {
                    let mut bytes = Vec::new();
                    file.read_to_end(&mut bytes).map(|_| bytes)
                },
            );
        match opened {
            Ok(bytes) => {
                return match parse(&bytes) {
                    Ok(value) => LoadOutcome::Loaded(value),
                    Err(error) => {
                        log::warn!("{} is corrupt: {error}", path.display());
                        LoadOutcome::Corrupt(error)
                    }
                };
            }
            Err(error) if is_not_found(&error) => return LoadOutcome::Missing,
            Err(error) => {
                if attempt < SETTINGS_FILE_REPLACE_RETRY_COUNT {
                    attempt += 1;
                    std::thread::sleep(Duration::from_millis(SETTINGS_FILE_REPLACE_RETRY_DELAY_MS));
                    continue;
                }
                log::warn!("{} is locked after {attempt} attempts: {error}", path.display());
                return LoadOutcome::Locked(error);
            }
        }
    }
}

/// UTC `yyyyMMddHHmmss` of `time`.
fn utc_stamp(time: SystemTime) -> String {
    let seconds = time.duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs()) as i64;
    let days = seconds.div_euclid(86_400);
    let second_of_day = seconds.rem_euclid(86_400);
    // Howard Hinnant's civil-from-days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}{month:02}{day:02}{:02}{:02}{:02}",
        second_of_day / 3600,
        (second_of_day / 60) % 60,
        second_of_day % 60
    )
}

/// `DataFileLoader.MoveCorruptAside`: renames to `{path}.corrupt-{UTC yyyyMMddHHmmss}` (plus
/// `-1`, `-2`… when taken), never overwriting. Returns the backup path, or None when the move
/// failed (the caller then leaves the file alone).
pub fn move_corrupt_aside(path: &Path, utc_now: SystemTime) -> Option<PathBuf> {
    let stamp = utc_stamp(utc_now);
    let base = path.as_os_str().to_owned();
    let mut backup = base.clone();
    backup.push(format!(".corrupt-{stamp}"));
    let mut suffix = 1;
    while Path::new(&backup).exists() {
        backup = base.clone();
        backup.push(format!(".corrupt-{stamp}-{suffix}"));
        suffix += 1;
    }
    let backup = PathBuf::from(backup);
    match move_file_no_replace(path, &backup) {
        Ok(()) => {
            log::warn!("{} was moved aside to {}; defaults are used instead.", path.display(), backup.display());
            Some(backup)
        }
        Err(error) => {
            log::warn!("{} is corrupt and could not be moved aside: {error}", path.display());
            None
        }
    }
}

fn wide(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str().encode_wide().chain(std::iter::once(0)).collect()
}

fn move_file(from: &Path, to: &Path, replace: bool) -> io::Result<()> {
    use windows::Win32::Storage::FileSystem::{MOVEFILE_COPY_ALLOWED, MOVEFILE_REPLACE_EXISTING, MoveFileExW};
    use windows_core::PCWSTR;
    let from = wide(from);
    let to = wide(to);
    let flags = if replace { MOVEFILE_REPLACE_EXISTING | MOVEFILE_COPY_ALLOWED } else { MOVEFILE_COPY_ALLOWED };
    // SAFETY: both buffers are NUL-terminated UTF-16 paths alive for the duration of the call.
    unsafe { MoveFileExW(PCWSTR(from.as_ptr()), PCWSTR(to.as_ptr()), flags) }
        .map_err(|error| io::Error::from_raw_os_error(error.code().0 & 0xFFFF))
}

fn move_file_no_replace(from: &Path, to: &Path) -> io::Result<()> {
    move_file(from, to, false)
}

// ---------------------------------------------------------------------------------------------
// AtomicFile
// ---------------------------------------------------------------------------------------------

/// One gate per destination (full path, case-insensitive like NTFS): replacements of one file
/// inside this process take turns.
fn replace_gate(path: &Path) -> Arc<Mutex<()>> {
    static GATES: LazyLock<Mutex<HashMap<String, Arc<Mutex<()>>>>> = LazyLock::new(Default::default);
    let key = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf()).to_string_lossy().to_lowercase();
    let mut gates = lock(&GATES);
    gates.entry(key).or_default().clone()
}

fn lock<V>(mutex: &Mutex<V>) -> MutexGuard<'_, V> {
    mutex.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn temporary_name() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_nanos()) as u64;
    let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
    let thread = std::thread::current().id();
    let mut hasher = std::hash::DefaultHasher::new();
    std::hash::Hash::hash(&(nanos, unique, thread), &mut hasher);
    let high = std::hash::Hasher::finish(&hasher);
    format!("{}.{high:016x}{:016x}.tmp", std::process::id(), nanos ^ unique.rotate_left(32))
}

/// Writes `bytes` to a temp file beside `path`, then replaces `path` (`AtomicFile`): the folder is
/// created, the temp file `{path}.{pid}.{unique}.tmp` is written and flushed to disk, and the move
/// is retried while another process briefly holds the destination. A failed write leaves no temp
/// file behind.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(directory) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(directory)?;
    }
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".");
    temporary.push(temporary_name());
    let temporary = PathBuf::from(temporary);
    let result = (|| {
        let mut file = OpenOptions::new().write(true).create_new(true).share_mode(0).open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        replace_with_retry(&temporary, path)
    })();
    if result.is_err() {
        delete_temporary_file(&temporary);
    }
    result
}

/// `AtomicFile.ReplaceWithRetry`.
fn replace_with_retry(temporary: &Path, path: &Path) -> io::Result<()> {
    let gate = replace_gate(path);
    let _turn = lock(&gate);
    let mut attempt = 1;
    loop {
        match move_file(temporary, path, true) {
            Ok(()) => return Ok(()),
            Err(error) if attempt < SETTINGS_FILE_REPLACE_RETRY_COUNT && !is_not_found(&error) => {
                attempt += 1;
                std::thread::sleep(Duration::from_millis(SETTINGS_FILE_REPLACE_RETRY_DELAY_MS));
            }
            Err(error) => return Err(error),
        }
    }
}

fn delete_temporary_file(temporary: &Path) {
    let mut attempt = 1;
    loop {
        match std::fs::remove_file(temporary) {
            Ok(()) => return,
            Err(error) if is_not_found(&error) => return,
            Err(error) => {
                if attempt < SETTINGS_FILE_REPLACE_RETRY_COUNT {
                    attempt += 1;
                    std::thread::sleep(Duration::from_millis(SETTINGS_FILE_REPLACE_RETRY_DELAY_MS));
                    continue;
                }
                log::warn!("AtomicFile: temporary file cleanup failed ({}): {error}", temporary.display());
                return;
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Store (TrayXmlFileStore)
// ---------------------------------------------------------------------------------------------

#[derive(Clone)]
struct WriteFailure {
    kind: io::ErrorKind,
    message: String,
}

struct StoreState<T> {
    /// Value of a debounced save and when the quiet period ends.
    scheduled: Option<(T, Instant)>,
    /// Latest serialized snapshot waiting for the writer, with its sequence number.
    pending: Option<(Vec<u8>, u64)>,
    next_sequence: u64,
    /// Highest sequence the writer finished (written or finally failed), and that write's result.
    completed: u64,
    completed_result: Option<WriteFailure>,
    writing: bool,
    shutdown: bool,
}

struct StoreShared<T> {
    path: PathBuf,
    state: Mutex<StoreState<T>>,
    changed: Condvar,
}

/// One writer per file. Implicit saves are debounced (400 ms, latest value wins) and written on a
/// background thread; explicit saves write synchronously.
pub struct XmlFileStore<T: XmlDocument> {
    path: PathBuf,
    read_only: bool,
    shared: Arc<StoreShared<T>>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl<T: XmlDocument> XmlFileStore<T> {
    /// Loads the file (with the missing/corrupt/locked rules) and returns the store, the value and
    /// how loading went. A missing file is created from defaults immediately; a loaded file whose
    /// normalization changed it is saved once.
    pub fn open(path: PathBuf) -> (Self, T, LoadState) {
        let outcome = try_load(&path, |bytes| {
            let root = parse_root::<T>(bytes)?;
            T::read_normalized(&root)
        });
        let (value, state, save, read_only) = match outcome {
            LoadOutcome::Loaded((value, changed)) => (value, LoadState::Loaded, changed, false),
            LoadOutcome::Missing => {
                let mut value = T::default();
                value.normalize();
                (value, LoadState::Missing, true, false)
            }
            LoadOutcome::Corrupt(_) => {
                let mut value = T::default();
                value.normalize();
                let backup = move_corrupt_aside(&path, SystemTime::now());
                let read_only = backup.is_none();
                (value, LoadState::Corrupt { backup }, false, read_only)
            }
            LoadOutcome::Locked(_) => {
                let mut value = T::default();
                value.normalize();
                (value, LoadState::Locked, false, true)
            }
        };
        if read_only {
            log::warn!(
                "{}: saving is disabled for this session so the existing file is not overwritten.",
                path.display()
            );
        }
        let store = Self::with_state(path, read_only);
        if save && let Err(error) = store.save_now(&value) {
            log::error!("{}: save failed: {error}", store.path.display());
        }
        (store, value, state)
    }

    fn with_state(path: PathBuf, read_only: bool) -> Self {
        let shared = Arc::new(StoreShared {
            path: path.clone(),
            state: Mutex::new(StoreState {
                scheduled: None,
                pending: None,
                next_sequence: 1,
                completed: 0,
                completed_result: None,
                writing: false,
                shutdown: false,
            }),
            changed: Condvar::new(),
        });
        Self { path, read_only, shared, worker: Mutex::new(None) }
    }

    fn ensure_worker(&self) {
        let mut worker = lock(&self.worker);
        if worker.is_some() {
            return;
        }
        let shared = Arc::clone(&self.shared);
        let name = format!(
            "xml-store:{}",
            self.path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned())
        );
        match std::thread::Builder::new().name(name).spawn(move || write_loop(&shared)) {
            Ok(handle) => *worker = Some(handle),
            Err(error) => log::error!("{}: could not start the save thread: {error}", self.path.display()),
        }
    }

    /// Debounced background save of `value` (no-op when read-only). Each call restarts the quiet
    /// period; the latest value is written when it ends.
    pub fn schedule_save(&self, value: &T) {
        if self.read_only {
            return;
        }
        self.ensure_worker();
        let deadline = Instant::now() + Duration::from_millis(SETTINGS_SAVE_DEBOUNCE_MS);
        let mut state = lock(&self.shared.state);
        state.scheduled = Some((value.clone(), deadline));
        self.shared.changed.notify_all();
    }

    /// Immediate save (no-op when read-only): cancels a scheduled save, serializes on the caller's
    /// thread and waits until the writer has replaced the file.
    pub fn save_now(&self, value: &T) -> io::Result<()> {
        if self.read_only {
            return Ok(());
        }
        let bytes = serialize(value);
        self.ensure_worker();
        let mut state = lock(&self.shared.state);
        state.scheduled = None;
        let sequence = queue_write(&mut state, bytes);
        self.shared.changed.notify_all();
        while state.completed < sequence {
            if self.worker_missing() {
                return Err(io::Error::other("the save thread is not running"));
            }
            state = self.shared.changed.wait(state).unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        match &state.completed_result {
            None => Ok(()),
            Some(failure) => Err(io::Error::new(failure.kind, failure.message.clone())),
        }
    }

    fn worker_missing(&self) -> bool {
        lock(&self.worker).is_none()
    }

    /// Blocks until any scheduled save has been written: a pending debounce is written now, then
    /// every queued write is awaited.
    pub fn flush(&self) {
        if self.worker_missing() {
            return;
        }
        let mut state = lock(&self.shared.state);
        if let Some((value, _)) = state.scheduled.take() {
            drop(state);
            let bytes = serialize(&value);
            state = lock(&self.shared.state);
            queue_write(&mut state, bytes);
            self.shared.changed.notify_all();
        }
        while state.pending.is_some() || state.writing || state.scheduled.is_some() {
            state = self.shared.changed.wait(state).unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// True after a Locked load (or a corrupt file that could not be moved aside): nothing is
    /// written this session.
    pub fn is_read_only(&self) -> bool {
        self.read_only
    }
}

impl<T: XmlDocument> Drop for XmlFileStore<T> {
    /// Writes queued snapshots (a still-debounced save is flushed too) and stops the writer.
    fn drop(&mut self) {
        let Some(handle) = lock(&self.worker).take() else { return };
        {
            let mut state = lock(&self.shared.state);
            if let Some((value, _)) = state.scheduled.take() {
                queue_write(&mut state, serialize(&value));
            }
            state.shutdown = true;
            self.shared.changed.notify_all();
        }
        if handle.join().is_err() {
            log::error!("{}: the save thread panicked", self.path.display());
        }
    }
}

fn queue_write<T>(state: &mut StoreState<T>, bytes: Vec<u8>) -> u64 {
    let sequence = state.next_sequence;
    state.next_sequence += 1;
    state.pending = Some((bytes, sequence));
    sequence
}

/// The store's writer: serializes due debounced saves, writes the latest snapshot, and retries a
/// failed snapshot (unless a newer one replaced it) for up to [`SETTINGS_FILE_REPLACE_RETRY_COUNT`]
/// rounds, sleeping `50 ms × round`.
fn write_loop<T: XmlDocument>(shared: &StoreShared<T>) {
    let mut failed_rounds = 0u32;
    let mut state = lock(&shared.state);
    loop {
        if state.pending.is_none()
            && let Some((_, deadline)) = &state.scheduled
        {
            let now = Instant::now();
            if *deadline <= now {
                if let Some((value, _)) = state.scheduled.take() {
                    drop(state);
                    let bytes = serialize(&value);
                    state = lock(&shared.state);
                    // A save_now that arrived meanwhile is newer than this snapshot.
                    if state.pending.is_none() {
                        queue_write(&mut state, bytes);
                    }
                }
            } else {
                let wait = *deadline - now;
                state = shared.changed.wait_timeout(state, wait).unwrap_or_else(std::sync::PoisonError::into_inner).0;
                continue;
            }
        }
        let Some((bytes, sequence)) = state.pending.take() else {
            if state.shutdown {
                return;
            }
            state = shared.changed.wait(state).unwrap_or_else(std::sync::PoisonError::into_inner);
            continue;
        };
        state.writing = true;
        drop(state);
        let result = atomic_write(&shared.path, &bytes);
        state = lock(&shared.state);
        state.writing = false;
        match result {
            Ok(()) => {
                failed_rounds = 0;
                state.completed = state.completed.max(sequence);
                state.completed_result = None;
            }
            Err(error) => {
                failed_rounds += 1;
                let retry = failed_rounds < SETTINGS_FILE_REPLACE_RETRY_COUNT;
                if retry {
                    log::warn!("{}: save attempt {failed_rounds} failed, retrying: {error}", shared.path.display());
                    if state.pending.is_none() {
                        state.pending = Some((bytes, sequence));
                    } else {
                        state.completed = state.completed.max(sequence);
                    }
                    shared.changed.notify_all();
                    drop(state);
                    std::thread::sleep(Duration::from_millis(
                        SETTINGS_FILE_REPLACE_RETRY_DELAY_MS * u64::from(failed_rounds),
                    ));
                    state = lock(&shared.state);
                    continue;
                }
                log::error!("{}: save failed: {error}", shared.path.display());
                failed_rounds = 0;
                state.completed = state.completed.max(sequence);
                state.completed_result = Some(WriteFailure { kind: error.kind(), message: error.to_string() });
            }
        }
        shared.changed.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug, Default, PartialEq)]
    struct Doc {
        name: String,
        count: i32,
        ratio: f64,
        flag: bool,
        items: Vec<String>,
    }

    impl XmlDocument for Doc {
        const ROOT: &'static str = "Doc";

        fn read_xml(root: &XmlElement) -> Self {
            Self::try_read_xml(root).unwrap_or_default()
        }

        fn try_read_xml(root: &XmlElement) -> Result<Self, XmlError> {
            let mut doc = Doc { count: 7, ..Doc::default() };
            if let Some(name) = root.attr("Name") {
                name.clone_into(&mut doc.name);
            }
            for e in &root.children {
                match e.name.as_str() {
                    "Count" => e.read_i32(&mut doc.count)?,
                    "Ratio" => e.read_f64(&mut doc.ratio)?,
                    "Flag" => e.read_bool(&mut doc.flag)?,
                    "Item" => doc.items.push(e.content()?.to_owned()),
                    _ => {}
                }
            }
            Ok(doc)
        }

        fn write_xml(&self, w: &mut XmlWriter) {
            w.attribute("Name", &self.name);
            w.element_i32("Count", self.count);
            w.element_f64("Ratio", self.ratio);
            w.element_bool("Flag", self.flag);
            for item in &self.items {
                w.element("Item", item);
            }
        }
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tadn-xml-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap_or_default();
        dir
    }

    fn files_in(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .map(|entries| {
                entries.filter_map(Result::ok).map(|e| e.file_name().to_string_lossy().into_owned()).collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    #[test]
    fn doubles_format_like_xml_convert() {
        for (value, text) in [
            (0.0, "0"),
            (-0.0, "-0"),
            (1.0, "1"),
            (2.5, "2.5"),
            (-12.0, "-12"),
            (5417.0, "5417"),
            (0.1, "0.1"),
            (123.456, "123.456"),
            (1e14, "100000000000000"),
            (1e15, "1E+15"),
            (123_456_789_012_345_680.0, "1.2345678901234568E+17"),
            (0.0001, "0.0001"),
            (0.00001, "1E-05"),
            (1.5e-10, "1.5E-10"),
            (f64::NAN, "NaN"),
            (f64::INFINITY, "INF"),
            (f64::NEG_INFINITY, "-INF"),
            (1.0 / 3.0, "0.3333333333333333"),
        ] {
            assert_eq!(format_f64(value), text, "{value}");
        }
    }

    #[test]
    fn scalars_parse_like_xml_convert() {
        assert_eq!(parse_f64(" 2.5 "), Some(2.5));
        assert_eq!(parse_f64("1E+15"), Some(1e15));
        assert_eq!(parse_f64("-INF"), Some(f64::NEG_INFINITY));
        assert_eq!(parse_f64(".5"), Some(0.5));
        assert_eq!(parse_f64("5."), Some(5.0));
        assert!(parse_f64("NaN").is_some_and(f64::is_nan));
        assert_eq!(parse_f64("inf"), None);
        assert_eq!(parse_f64("1,5"), None);
        assert_eq!(parse_f64(""), None);
        assert_eq!(parse_bool("1"), Some(true));
        assert_eq!(parse_bool(" false "), Some(false));
        assert_eq!(parse_bool("True"), None);
        assert_eq!(parse_i32("+5"), Some(5));
        assert_eq!(parse_i32(" -3 "), Some(-3));
        assert_eq!(parse_i32("3.0"), None);
        assert_eq!(parse_i32("99999999999"), None);
        assert_eq!(parse_u32("-0"), Some(0));
        assert_eq!(parse_u32("-1"), None);
    }

    #[test]
    fn writer_matches_dotnet_layout() {
        let doc = Doc {
            name: "a\"<b>&'".to_owned(),
            count: 3,
            ratio: 0.5,
            flag: true,
            items: vec![String::new(), "x\ny".to_owned()],
        };
        let text = String::from_utf8(serialize(&doc)).unwrap_or_default();
        assert_eq!(
            text,
            "\u{FEFF}<?xml version=\"1.0\" encoding=\"utf-8\"?>\r\n<Doc Name=\"a&quot;&lt;b&gt;&amp;'\">\r\n  <Count>3</Count>\r\n  \
             <Ratio>0.5</Ratio>\r\n  <Flag>true</Flag>\r\n  <Item />\r\n  <Item>x\r\ny</Item>\r\n</Doc>"
        );
        let mut empty = XmlWriter::new();
        empty.start_element("Root");
        empty.start_element("Wrapper");
        empty.end_element();
        empty.end_element();
        assert!(String::from_utf8(empty.finish()).unwrap_or_default().ends_with("<Root>\r\n  <Wrapper />\r\n</Root>"));
    }

    #[test]
    fn characters_xml_cannot_carry_are_dropped() {
        let doc = Doc { items: vec!["My\u{1} Headphones \u{1F3A7}\u{FFFF}".to_owned()], ..Doc::default() };
        let back: Doc = deserialize(&serialize(&doc)).unwrap_or_default();
        assert_eq!(back.items, vec!["My Headphones \u{1F3A7}".to_owned()]);
    }

    #[test]
    fn unknown_elements_skipped_and_invalid_values_keep_defaults() {
        let xml = "<Doc Name=\"n\"><Count>not-an-int</Count><Unknown>skip <b>me</b></Unknown><!-- c --><Flag>1</Flag>\
                   <Ratio>  </Ratio><Item>  kept  </Item></Doc>";
        let doc: Doc = deserialize(xml.as_bytes()).unwrap_or_default();
        assert_eq!(
            doc,
            Doc { name: "n".to_owned(), count: 7, ratio: 0.0, flag: true, items: vec!["  kept  ".to_owned()] }
        );
    }

    #[test]
    fn wrong_root_dtd_and_nested_scalars_are_corrupt() {
        assert!(deserialize::<Doc>(b"<Other />").is_err());
        assert!(deserialize::<Doc>(b"<!DOCTYPE Doc [<!ENTITY x \"y\">]><Doc />").is_err());
        assert!(deserialize::<Doc>(b"<Doc><Count><x/></Count></Doc>").is_err());
        assert!(deserialize::<Doc>(b"<Doc><Count>1</Count>").is_err());
        assert!(deserialize::<Doc>(b"\xEF\xBB\xBF<Doc><Count>1</Count></Doc>").is_ok_and(|d| d.count == 1));
    }

    #[test]
    fn utc_stamp_is_yyyymmddhhmmss() {
        let time = SystemTime::UNIX_EPOCH + Duration::from_secs(1_782_045_296); // 2026-06-21 12:34:56 UTC
        assert_eq!(utc_stamp(time), "20260621123456");
        assert_eq!(utc_stamp(SystemTime::UNIX_EPOCH + Duration::from_secs(951_782_400)), "20000229000000");
    }

    #[test]
    fn missing_file_is_created_from_defaults() {
        let dir = temp_dir("missing");
        let path = dir.join("settings.xml");
        let (store, value, state) = XmlFileStore::<Doc>::open(path.clone());
        store.flush();
        assert_eq!(state, LoadState::Missing);
        assert!(!store.is_read_only());
        assert_eq!(std::fs::read(&path).unwrap_or_default(), serialize(&value));
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_file_is_moved_to_a_backup_and_never_overwritten() {
        let dir = temp_dir("corrupt");
        let path = dir.join("settings.xml");
        let corrupt = "<Doc><Count>3</Count";
        std::fs::write(&path, corrupt).unwrap_or_default();
        let (store, value, state) = XmlFileStore::<Doc>::open(path.clone());
        store.flush();
        assert_eq!(value, Doc::default());
        let LoadState::Corrupt { backup: Some(backup) } = state else {
            panic!("expected a corrupt load with a backup")
        };
        assert!(backup.file_name().is_some_and(|n| n.to_string_lossy().starts_with("settings.xml.corrupt-")));
        assert_eq!(std::fs::read_to_string(&backup).unwrap_or_default(), corrupt);
        assert!(!path.exists());
        assert!(!store.is_read_only());
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn locked_file_runs_on_defaults_with_saving_disabled() {
        let dir = temp_dir("locked");
        let path = dir.join("settings.xml");
        let content = "<Doc><Count>3</Count></Doc>";
        std::fs::write(&path, content).unwrap_or_default();
        let holder = OpenOptions::new().read(true).write(true).share_mode(0).open(&path);
        assert!(holder.is_ok());
        let (store, mut value, state) = XmlFileStore::<Doc>::open(path.clone());
        assert_eq!(state, LoadState::Locked);
        assert!(store.is_read_only());
        value.count = 9;
        assert!(store.save_now(&value).is_ok());
        store.schedule_save(&value);
        store.flush();
        drop(holder);
        drop(store);
        assert_eq!(std::fs::read_to_string(&path).unwrap_or_default(), content);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scheduled_saves_are_debounced_and_latest_wins() {
        let dir = temp_dir("debounce");
        let path = dir.join("settings.xml");
        let (store, mut value, _) = XmlFileStore::<Doc>::open(path.clone());
        store.flush();
        let initial = std::fs::read(&path).unwrap_or_default();
        for count in 1..=5 {
            value.count = count;
            store.schedule_save(&value);
        }
        assert_eq!(std::fs::read(&path).unwrap_or_default(), initial, "nothing written before the quiet period");
        std::thread::sleep(Duration::from_millis(SETTINGS_SAVE_DEBOUNCE_MS + 400));
        let written: Doc = deserialize(&std::fs::read(&path).unwrap_or_default()).unwrap_or_default();
        assert_eq!(written.count, 5);

        value.count = 42;
        store.schedule_save(&value);
        store.flush();
        let flushed: Doc = deserialize(&std::fs::read(&path).unwrap_or_default()).unwrap_or_default();
        assert_eq!(flushed.count, 42);

        value.count = 43;
        store.schedule_save(&value);
        drop(store);
        let dropped: Doc = deserialize(&std::fs::read(&path).unwrap_or_default()).unwrap_or_default();
        assert_eq!(dropped.count, 43);
        assert_eq!(files_in(&dir), vec!["settings.xml".to_owned()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn loading_an_existing_file_schedules_no_save() {
        let dir = temp_dir("nosave");
        let path = dir.join("settings.xml");
        std::fs::write(&path, "<Doc><Count>3</Count></Doc>").unwrap_or_default();
        let (store, value, state) = XmlFileStore::<Doc>::open(path.clone());
        store.flush();
        drop(store);
        assert_eq!((state, value.count), (LoadState::Loaded, 3));
        assert_eq!(std::fs::read_to_string(&path).unwrap_or_default(), "<Doc><Count>3</Count></Doc>");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn concurrent_atomic_writers_leave_one_complete_version() {
        let dir = temp_dir("atomic");
        let path = dir.join("data").join("data.txt");
        let barrier = Arc::new(std::sync::Barrier::new(64));
        let writers: Vec<_> = (0..64)
            .map(|index| {
                let path = path.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    let contents = format!("writer {index} {}", "x".repeat(64 * 1024));
                    barrier.wait();
                    atomic_write(&path, contents.as_bytes())
                })
            })
            .collect();
        for writer in writers {
            assert!(writer.join().is_ok_and(|r| r.is_ok()));
        }
        let written = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(written.starts_with("writer "));
        assert_eq!(written.len() - written.find('x').unwrap_or(0), 64 * 1024);
        assert_eq!(files_in(&dir.join("data")), vec!["data.txt".to_owned()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_write_leaves_no_temporary_file() {
        let dir = temp_dir("failed");
        let target = dir.join("target");
        std::fs::create_dir_all(target.join("child")).unwrap_or_default();
        assert!(atomic_write(&target, b"data").is_err());
        assert_eq!(files_in(&dir), vec!["target".to_owned()]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
