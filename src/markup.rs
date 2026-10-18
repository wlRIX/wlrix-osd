// SPDX-License-Identifier: GPL-3.0-or-later
//! The body markup the specification allows, and nothing else.
//!
//! A notification body may carry `<b>`, `<i>`, `<u>`, `<a href="…">` and `<img src="…"
//! alt="…"/>`, and the five XML entities. It is *not* HTML and it is not even reliably XML:
//! senders forget to escape a `<` in "x < y", leave tags unclosed, and send `<br>` because it
//! works in every other notification daemon. So this is a forgiving scanner rather than a
//! parser, and its one rule is that **text is never lost**:
//!
//! - A `<` that does not open something shaped like a tag is a literal `<`.
//! - A tag this does not know is dropped and its contents kept.
//! - An `&` that does not start a known entity is a literal `&`.
//! - A closing tag with no opener is ignored; an opener with no closer runs to the end.
//!
//! What comes out is a run of [`Span`]s, each one piece of text in one [`Style`], with line
//! breaks as `\n` inside the text. The painter never sees a tag.
//!
//! # What the styles become
//!
//! `wlrix-ui` has three faces -- regular, bold, italic -- and no bold italic, so a span that
//! is both draws bold. Links draw in the scheme's link color, underlined; clicking one does
//! nothing yet, which is why `body-hyperlinks` is not among the advertised capabilities.
//! Images are not drawn either, and `body-images` is not advertised: an `<img>` becomes its
//! `alt` text, which is what the specification asks of a server that cannot show it.

/// How a run of text is drawn.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub link: bool,
}

/// One run of text in one style.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub style: Style,
}

/// How many levels of the same tag are tracked. `<b><b><b>` is legal enough, and a counter
/// rather than a flag means the first `</b>` does not turn bold off under the other two -- but
/// a body that opens a tag a thousand times is not formatting, and a ceiling keeps the counters
/// honest.
const MAX_DEPTH: u32 = 32;

/// Read a body into spans.
pub fn parse(body: &str) -> Vec<Span> {
    let mut spans: Vec<Span> = Vec::new();
    let mut depth = Depth::default();
    let mut text = String::new();
    let mut rest = body;

    while let Some(at) = rest.find(['<', '&']) {
        text.push_str(&rest[..at]);
        rest = &rest[at..];
        if rest.starts_with('&') {
            let (decoded, used) = entity(rest);
            text.push_str(&decoded);
            rest = &rest[used..];
            continue;
        }
        let Some(tag) = Tag::read(rest) else {
            text.push('<');
            rest = &rest[1..];
            continue;
        };
        rest = &rest[tag.length..];

        // A tag changes the style from here on, so whatever came before it is finished.
        let before = depth.style();
        match (tag.name.as_str(), tag.closing) {
            ("b", closing) => depth.bold = step(depth.bold, closing),
            ("i", closing) => depth.italic = step(depth.italic, closing),
            ("u", closing) => depth.underline = step(depth.underline, closing),
            ("a", closing) => depth.link = step(depth.link, closing),
            ("br", _) => text.push('\n'),
            ("img", false) => {
                if let Some(alt) = tag.attribute("alt") {
                    text.push_str(&alt);
                }
            }
            // Anything else is dropped and its contents kept. `<p>` and `<div>` are the common
            // ones, and they get no line break: a sender that relies on them for layout is
            // relying on HTML, and a body run together is still readable.
            _ => {}
        }
        if depth.style() != before {
            push(&mut spans, std::mem::take(&mut text), before);
        }
    }
    text.push_str(rest);
    push(&mut spans, text, depth.style());
    spans
}

/// The text with every tag taken out, for a log line or anywhere one style is all there is.
pub fn plain(spans: &[Span]) -> String {
    spans.iter().map(|span| span.text.as_str()).collect()
}

/// How deep inside each tag the scanner is.
#[derive(Default)]
struct Depth {
    bold: u32,
    italic: u32,
    underline: u32,
    link: u32,
}

impl Depth {
    fn style(&self) -> Style {
        Style {
            bold: self.bold > 0,
            italic: self.italic > 0,
            underline: self.underline > 0,
            link: self.link > 0,
        }
    }
}

/// One more or one fewer, never below zero -- a stray `</b>` closes nothing.
fn step(depth: u32, closing: bool) -> u32 {
    if closing {
        depth.saturating_sub(1)
    } else {
        (depth + 1).min(MAX_DEPTH)
    }
}

/// Add a span, merging it into the last one when the style did not actually change.
fn push(spans: &mut Vec<Span>, text: String, style: Style) {
    if text.is_empty() {
        return;
    }
    if let Some(last) = spans.last_mut()
        && last.style == style
    {
        last.text.push_str(&text);
        return;
    }
    spans.push(Span { text, style });
}

/// A tag, as far as this module cares: its name, whether it closes, and its attributes as raw
/// text.
struct Tag {
    name: String,
    closing: bool,
    attributes: String,
    /// How many bytes of the input it took, `<` and `>` included.
    length: usize,
}

impl Tag {
    /// Read a tag at the start of `text`, which begins with `<`.
    ///
    /// Something is a tag only if it looks like one: `<`, an optional `/`, a letter, and a `>`
    /// before the next `<` or the end of the line. "x <y" and "a < b" are text, and so is a
    /// `<` with no `>` after it at all -- turning the rest of a body into one unterminated tag
    /// would lose all of it.
    fn read(text: &str) -> Option<Self> {
        let inner = &text[1..];
        let end = inner.find(['>', '<', '\n'])?;
        if inner.as_bytes()[end] != b'>' {
            return None;
        }
        let body = &inner[..end];
        let (closing, body) = match body.strip_prefix('/') {
            Some(body) => (true, body),
            None => (false, body),
        };
        if !body.starts_with(|c: char| c.is_ascii_alphabetic()) {
            return None;
        }
        let name_end = body
            .find(|c: char| !c.is_ascii_alphanumeric())
            .unwrap_or(body.len());
        Some(Self {
            name: body[..name_end].to_ascii_lowercase(),
            closing,
            attributes: body[name_end..].trim_end_matches('/').to_owned(),
            length: end + 2,
        })
    }

    /// One attribute's value, quoted with either quote or not at all, entities decoded.
    fn attribute(&self, wanted: &str) -> Option<String> {
        let mut rest = self.attributes.as_str();
        loop {
            rest = rest.trim_start();
            if rest.is_empty() {
                return None;
            }
            let name_end = rest.find(|c: char| c == '=' || c.is_whitespace())?;
            let name = &rest[..name_end];
            rest = rest[name_end..].trim_start();
            let Some(after) = rest.strip_prefix('=') else {
                // A bare attribute, like `<img ismap>`: no value to take.
                continue;
            };
            rest = after.trim_start();
            let (value, used) = match rest.chars().next() {
                Some(quote @ ('"' | '\'')) => {
                    let close = rest[1..].find(quote).map_or(rest.len(), |at| at + 1);
                    (&rest[1..close], (close + 1).min(rest.len()))
                }
                _ => {
                    let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
                    (&rest[..end], end)
                }
            };
            if name.eq_ignore_ascii_case(wanted) {
                return Some(decode(value));
            }
            rest = &rest[used..];
        }
    }
}

/// Decode every entity in `text`.
fn decode(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        let (decoded, used) = entity(&rest[at..]);
        out.push_str(&decoded);
        rest = &rest[at + used..];
    }
    out.push_str(rest);
    out
}

/// The entity at the start of `text`, which begins with `&`, and how many bytes it took.
///
/// An `&` that starts nothing recognizable is itself: one byte, a literal ampersand. Numeric
/// references decode to their character unless that character is a control code or not a
/// character at all, in which case the reference stays as written.
fn entity(text: &str) -> (String, usize) {
    let literal = ("&".to_owned(), 1);
    // Entities are short; looking further than this for the `;` would let an `&` in the middle
    // of a sentence swallow the next one's text.
    let Some(end) = text
        .get(..text.len().min(12))
        .and_then(|head| head.find(';'))
    else {
        return literal;
    };
    let name = &text[1..end];
    let named = match name {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        "nbsp" => Some('\u{a0}'),
        _ => None,
    };
    let numeric = || {
        let digits = name.strip_prefix('#')?;
        let value = match digits.strip_prefix(['x', 'X']) {
            Some(hex) => u32::from_str_radix(hex, 16).ok()?,
            None => digits.parse().ok()?,
        };
        char::from_u32(value).filter(|c| !c.is_control() || *c == '\n')
    };
    match named.or_else(numeric) {
        Some(c) => (c.to_string(), end + 1),
        None => literal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(text: &str, style: Style) -> Span {
        Span {
            text: text.to_owned(),
            style,
        }
    }

    const PLAIN: Style = Style {
        bold: false,
        italic: false,
        underline: false,
        link: false,
    };
    const BOLD: Style = Style {
        bold: true,
        ..PLAIN
    };

    #[test]
    fn plain_text_is_one_span() {
        assert_eq!(
            parse("Download complete"),
            vec![span("Download complete", PLAIN)]
        );
        assert!(parse("").is_empty());
    }

    #[test]
    fn the_spec_tags_set_styles() {
        assert_eq!(
            parse("a <b>bold</b> and <i>it</i>"),
            vec![
                span("a ", PLAIN),
                span("bold", BOLD),
                span(" and ", PLAIN),
                span(
                    "it",
                    Style {
                        italic: true,
                        ..PLAIN
                    }
                ),
            ]
        );
        assert_eq!(
            parse("<a href=\"https://example.org\">link</a>"),
            vec![span(
                "link",
                Style {
                    link: true,
                    ..PLAIN
                }
            )]
        );
    }

    #[test]
    fn nested_tags_close_one_level_at_a_time() {
        let spans = parse("<b>one <b>two</b> still</b> done");
        assert_eq!(
            spans,
            vec![span("one two still", BOLD), span(" done", PLAIN)]
        );
    }

    #[test]
    fn a_stray_close_or_an_unclosed_open_loses_nothing() {
        assert_eq!(parse("text</b> more"), vec![span("text more", PLAIN)]);
        assert_eq!(
            parse("start <b>to the end"),
            vec![span("start ", PLAIN), span("to the end", BOLD)]
        );
    }

    #[test]
    fn a_less_than_that_is_not_a_tag_is_text() {
        // The commonest malformed body there is: a sender that did not escape a comparison.
        assert_eq!(parse("x < y"), vec![span("x < y", PLAIN)]);
        assert_eq!(parse("a <3 b"), vec![span("a <3 b", PLAIN)]);
        assert_eq!(parse("no close <b"), vec![span("no close <b", PLAIN)]);
        assert_eq!(parse("<<b>x</b>"), vec![span("<", PLAIN), span("x", BOLD)]);
    }

    #[test]
    fn unknown_tags_are_dropped_and_their_text_kept() {
        assert_eq!(
            parse("<p>para</p><span class=x>inner</span>"),
            vec![span("parainner", PLAIN)]
        );
        assert_eq!(
            parse("one<br>two<br/>three"),
            vec![span("one\ntwo\nthree", PLAIN)]
        );
    }

    #[test]
    fn an_image_becomes_its_alt_text() {
        assert_eq!(
            parse("see <img src=\"x.png\" alt=\"a cat\"/> here"),
            vec![span("see a cat here", PLAIN)]
        );
        assert_eq!(parse("<img src='x.png'/>"), Vec::<Span>::new());
        assert_eq!(parse("<img alt=bare src=x.png>"), vec![span("bare", PLAIN)]);
    }

    #[test]
    fn entities_decode_and_a_bare_ampersand_survives() {
        assert_eq!(
            parse("Tom &amp; Jerry &lt;3 &quot;hi&quot; &#65;&#x42;"),
            vec![span("Tom & Jerry <3 \"hi\" AB", PLAIN)]
        );
        assert_eq!(parse("R&D & more"), vec![span("R&D & more", PLAIN)]);
        assert_eq!(
            parse("&bogus; &#0; &#xZZ;"),
            vec![span("&bogus; &#0; &#xZZ;", PLAIN)]
        );
        assert_eq!(parse("&amp"), vec![span("&amp", PLAIN)]);
    }

    #[test]
    fn real_newlines_are_kept() {
        assert_eq!(parse("one\ntwo"), vec![span("one\ntwo", PLAIN)]);
    }

    #[test]
    fn tag_names_are_not_case_sensitive() {
        assert_eq!(parse("<B>x</B>"), vec![span("x", BOLD)]);
    }

    #[test]
    fn plain_strips_every_tag() {
        assert_eq!(plain(&parse("<b>a</b> <i>b</i>")), "a b");
    }
}
