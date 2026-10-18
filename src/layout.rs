// SPDX-License-Identifier: GPL-3.0-or-later
//! Where everything goes: inside one notification, and the notifications in the stack.
//!
//! # A notification is a 4Dwm window
//!
//! IRIX had no notification daemon, so there is no original to copy. The nearest native thing
//! is a small fixed-size window, and that is what each notification is drawn as: the full
//! beveled frame, a titlebar holding the application's name, and the message in the window's
//! body. A fixed-size window gets an unbroken *ring* for a border rather than the jointed one
//! with corner grips -- there is nothing to resize -- and no minimize or maximize buttons.
//!
//! The one button is the window-menu button, in its usual place at the left of the titlebar,
//! and here it closes the notification. That is the closest thing 4Dwm has to a close box:
//! double-clicking that button is how an IRIX window is closed.
//!
//! The metrics are the compositor's own (`wlrix-compositor/src/decoration.rs`): an 8-pixel
//! border, a 30-pixel titlebar, a 2-pixel bevel. A notification next to a real window should
//! look like a smaller one of the same.
//!
//! # Text
//!
//! The body is wrapped here rather than in `wlrix_ui::text::wrap`, which wraps a single run in
//! one face. A body can change face mid-line (`some <b>bold</b> words`), so it is wrapped a word
//! at a time, each measured in its own face. Measuring goes through [`Text`], so the layout is
//! testable with a fixed-width stand-in for the fonts.

use serde::Deserialize;
use wlrix_ui::canvas::Rect;
use wlrix_ui::text::Face;

use crate::markup::{Span, Style};

/// Border thickness on each side, outline included.
pub const BORDER: i32 = 8;
/// The titlebar's height, and the width of its one button.
pub const TITLEBAR: i32 = 30;
/// Bevel thickness of every frame piece.
pub const BEVEL: i32 = 2;
/// The 1-pixel keyline around the whole frame.
pub const OUTLINE: i32 = 1;
/// Space between the titlebar text and the pieces either side of it, as the compositor leaves.
const TITLE_PAD: i32 = 6;
/// Space around and between everything inside the window's body.
pub const PAD: i32 = 8;
/// Between the summary and the body.
const PARAGRAPH: i32 = 4;
/// The progress bar's height, bevel included, and the space above it.
pub const BAR: i32 = 14;
const BAR_GAP: i32 = 6;
/// The "N more" tab under a full stack.
pub const MORE: i32 = 24;

/// The title, the summary and the body.
pub const TITLE_PX: f32 = 15.0;
pub const SUMMARY_PX: f32 = 14.0;
pub const BODY_PX: f32 = 13.0;
/// The summary is a headline. Two lines of it is already a lot.
const SUMMARY_LINES: usize = 2;

/// What is drawn past text that was cut off.
pub const ELLIPSIS: &str = "…";

/// Which corner the stack grows from.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Anchor {
    #[default]
    TopRight,
    TopLeft,
    BottomRight,
    BottomLeft,
}

impl Anchor {
    pub fn is_top(self) -> bool {
        matches!(self, Anchor::TopLeft | Anchor::TopRight)
    }

    pub fn is_left(self) -> bool {
        matches!(self, Anchor::TopLeft | Anchor::BottomLeft)
    }
}

/// The font, as the layout needs it.
pub trait Text {
    /// How wide `text` is at this face and size.
    fn width(&mut self, face: Face, px: f32, text: &str) -> i32;
    /// One line's height at this face and size.
    fn line_height(&mut self, face: Face, px: f32) -> i32;
}

/// The face a style draws in. There is no bold italic in `wlrix-ui`, so bold wins.
pub fn face_of(style: Style) -> Face {
    if style.bold {
        Face::Bold
    } else if style.italic {
        Face::Italic
    } else {
        Face::Regular
    }
}

/// One stretch of a line in one style, where it starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Piece {
    pub x: i32,
    pub width: i32,
    pub text: String,
    pub style: Style,
}

/// One laid-out line: its pieces, left to right, and the top of its line box.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Line {
    pub top: i32,
    pub pieces: Vec<Piece>,
}

impl Line {
    fn end(&self) -> i32 {
        self.pieces.last().map_or(0, |piece| piece.x + piece.width)
    }
}

/// The window around one notification, in the notification's own coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chrome {
    /// The whole window, outline included.
    pub frame: Rect,
    /// The window-menu button, which closes it.
    pub button: Rect,
    /// The titlebar piece the title sits in.
    pub title: Rect,
    /// Where the title text may go.
    pub title_text: Rect,
    /// The window's body.
    pub client: Rect,
}

impl Chrome {
    /// The frame for a window `width` wide whose body is `body` tall.
    pub fn new(width: i32, body: i32) -> Self {
        let client_w = width - 2 * BORDER;
        let button = Rect::new(BORDER, BORDER, TITLEBAR.min(client_w), TITLEBAR);
        let title = Rect::new(button.x + button.w, BORDER, client_w - button.w, TITLEBAR);
        Self {
            frame: Rect::new(0, 0, width, BORDER + TITLEBAR + body + BORDER),
            button,
            title,
            title_text: Rect::new(
                title.x + TITLE_PAD,
                title.y,
                (title.w - 2 * TITLE_PAD).max(0),
                title.h,
            ),
            client: Rect::new(BORDER, BORDER + TITLEBAR, client_w, body),
        }
    }
}

/// One notification, laid out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bubble {
    pub chrome: Chrome,
    /// The application's name, cut to fit the titlebar.
    pub title: String,
    /// Where the picture goes, already centered in its slot. `None` with no picture.
    pub picture: Option<Rect>,
    pub summary: Vec<Line>,
    pub body: Vec<Line>,
    /// The progress bar, for a notification with a `value` hint.
    pub bar: Option<Rect>,
}

/// What [`Bubble::new`] needs to know about a notification.
pub struct Content<'a> {
    pub title: &'a str,
    pub summary: &'a str,
    pub body: &'a [Span],
    /// The picture's size, if there is one. Drawn in a square slot of `icon` pixels.
    pub picture: Option<(i32, i32)>,
    pub value: Option<i32>,
}

impl Bubble {
    /// Lay out one notification `width` wide, with a picture slot `icon` square and the body
    /// cut off after `body_lines`.
    pub fn new(
        content: &Content,
        width: i32,
        icon: i32,
        body_lines: usize,
        text: &mut dyn Text,
    ) -> Self {
        let client_w = width - 2 * BORDER;
        let has_picture = content.picture.is_some();
        let text_x = BORDER + PAD + if has_picture { icon + PAD } else { 0 };
        let text_w = (BORDER + client_w - PAD - text_x).max(1);

        let bold = Style {
            bold: true,
            ..Style::default()
        };
        let summary_span = [Span {
            text: content.summary.to_owned(),
            style: bold,
        }];
        let mut top = BORDER + TITLEBAR + PAD;
        let summary_height = text.line_height(Face::Bold, SUMMARY_PX);
        let mut summary = wrap(&summary_span, text_w, SUMMARY_LINES, SUMMARY_PX, text);
        for line in &mut summary {
            line.top = top;
            top += summary_height;
            offset(line, text_x);
        }

        let body_height = text.line_height(Face::Regular, BODY_PX);
        let mut body = wrap(content.body, text_w, body_lines, BODY_PX, text);
        // A body that is nothing but blank lines is no body.
        if body.iter().all(|line| line.pieces.is_empty()) {
            body.clear();
        }
        if !summary.is_empty() && !body.is_empty() {
            top += PARAGRAPH;
        }
        for line in &mut body {
            line.top = top;
            top += body_height;
            offset(line, text_x);
        }

        let bar = content.value.map(|_| {
            if !summary.is_empty() || !body.is_empty() {
                top += BAR_GAP;
            }
            let bar = Rect::new(text_x, top, text_w, BAR);
            top += BAR;
            bar
        });

        let text_bottom = top + PAD;
        let picture_bottom = BORDER + TITLEBAR + PAD + if has_picture { icon + PAD } else { 0 };
        let body_h = text_bottom.max(picture_bottom) - (BORDER + TITLEBAR);
        let chrome = Chrome::new(width, body_h);

        let picture = content.picture.map(|(w, h)| {
            let slot = Rect::new(BORDER + PAD, BORDER + TITLEBAR + PAD, icon, icon);
            Rect::new(
                slot.x + (slot.w - w.min(icon)) / 2,
                slot.y + (slot.h - h.min(icon)) / 2,
                w.min(icon),
                h.min(icon),
            )
        });

        Self {
            title: fit(
                content.title,
                chrome.title_text.w,
                Face::Bold,
                TITLE_PX,
                text,
            ),
            chrome,
            picture,
            summary,
            body,
            bar,
        }
    }

    pub fn height(&self) -> i32 {
        self.chrome.frame.h
    }
}

/// Move a line's pieces right by `x`.
fn offset(line: &mut Line, x: i32) {
    for piece in &mut line.pieces {
        piece.x += x;
    }
}

/// What a body breaks into before it is placed.
enum Token<'a> {
    Word(&'a str, Style),
    Space(Style),
    Break,
}

/// Split spans into words, runs of space and hard line breaks.
fn tokens(spans: &[Span]) -> Vec<Token<'_>> {
    let mut out = Vec::new();
    for span in spans {
        for (index, paragraph) in span.text.split('\n').enumerate() {
            if index > 0 {
                out.push(Token::Break);
            }
            let mut rest = paragraph;
            while !rest.is_empty() {
                let space = rest.len() - rest.trim_start().len();
                if space > 0 {
                    out.push(Token::Space(span.style));
                    rest = &rest[space..];
                    continue;
                }
                let word = rest.find(char::is_whitespace).unwrap_or(rest.len());
                out.push(Token::Word(&rest[..word], span.style));
                rest = &rest[word..];
            }
        }
    }
    out
}

/// Wrap styled text to `width`, at most `max_lines` lines, cutting the last one with an
/// ellipsis if anything was left over.
///
/// Breaks at spaces; a word wider than a whole line is broken between characters, since a URL
/// or a file name has no spaces to break at and would otherwise run off the edge. Runs of space
/// collapse to one, and a space at a line break is dropped. Lines come back with their pieces
/// starting at x = 0 and their `top` unset.
pub fn wrap(
    spans: &[Span],
    width: i32,
    max_lines: usize,
    px: f32,
    text: &mut dyn Text,
) -> Vec<Line> {
    let mut lines = vec![Line::default()];
    let mut pending_space: Option<Style> = None;
    let mut cut = false;

    'tokens: for token in tokens(spans) {
        match token {
            Token::Break => {
                if lines.len() == max_lines {
                    cut = true;
                    break;
                }
                lines.push(Line::default());
                pending_space = None;
            }
            Token::Space(style) => {
                if !lines.last().unwrap().pieces.is_empty() {
                    pending_space = Some(style);
                }
            }
            Token::Word(word, style) => {
                let face = face_of(style);
                let mut word = word;
                loop {
                    let line = lines.last_mut().unwrap();
                    let space = pending_space
                        .map(|style| text.width(face_of(style), px, " "))
                        .unwrap_or(0);
                    let word_w = text.width(face, px, word);
                    if line.end() + space + word_w <= width {
                        place(
                            line,
                            pending_space.take().map(|style| (style, space)),
                            word,
                            word_w,
                            style,
                            px,
                            text,
                        );
                        break;
                    }
                    if !line.pieces.is_empty() {
                        // Does not fit after what is there: a new line, without the space.
                        if lines.len() == max_lines {
                            cut = true;
                            break 'tokens;
                        }
                        lines.push(Line::default());
                        pending_space = None;
                        continue;
                    }
                    // Wider than a whole line on its own: take what fits and carry the rest.
                    let (head, tail) = split_to_fit(word, width, face, px, text);
                    let head_w = text.width(face, px, head);
                    place(line, None, head, head_w, style, px, text);
                    word = tail;
                    if word.is_empty() {
                        break;
                    }
                    if lines.len() == max_lines {
                        cut = true;
                        break 'tokens;
                    }
                    lines.push(Line::default());
                }
            }
        }
    }

    // Blank lines at the end are a sender's trailing newline, not content.
    while lines.len() > 1 && lines.last().unwrap().pieces.is_empty() {
        lines.pop();
    }
    if lines.len() == 1 && lines[0].pieces.is_empty() {
        return Vec::new();
    }
    if cut {
        ellipsize(lines.last_mut().unwrap(), width, px, text);
    }
    lines
}

/// Put a word at the end of a line, merging it into the last piece when the style matches so
/// a plain paragraph is one draw call per line rather than one per word.
///
/// A merged piece is measured again whole rather than having the word's width added on. Each
/// word measured alone is rounded up to a whole pixel, and the font kerns across a space it
/// never saw, so a sum of separate widths runs ahead of the drawn text by a pixel or so a word
/// -- enough, by the end of a line, to open a visible gap before the next piece.
///
/// A space between two pieces of different styles belongs to neither: it is the gap before the
/// second, so an underline on either side does not run through it.
fn place(
    line: &mut Line,
    space: Option<(Style, i32)>,
    word: &str,
    word_w: i32,
    style: Style,
    px: f32,
    text: &mut dyn Text,
) {
    let x = line.end();
    let space_w = space.map_or(0, |(_, width)| width);
    if let Some(last) = line.pieces.last_mut()
        && last.style == style
        && space.is_none_or(|(space, _)| space == style)
    {
        if space.is_some() {
            last.text.push(' ');
        }
        last.text.push_str(word);
        last.width = text.width(face_of(style), px, &last.text);
        return;
    }
    // Measured with the previous piece's own text, so the space is kerned against it rather
    // than rounded up on its own -- the same drift as above, at the join where it shows most.
    let x = match (space, line.pieces.last()) {
        (Some(_), Some(last)) => {
            last.x + text.width(face_of(last.style), px, &format!("{} ", last.text))
        }
        _ => x + space_w,
    };
    line.pieces.push(Piece {
        x,
        width: word_w,
        text: word.to_owned(),
        style,
    });
}

/// The longest prefix of `word` no wider than `width`, and the rest. Always at least one
/// character, so a line too narrow for any of it still makes progress.
fn split_to_fit<'a>(
    word: &'a str,
    width: i32,
    face: Face,
    px: f32,
    text: &mut dyn Text,
) -> (&'a str, &'a str) {
    let mut fit = word
        .char_indices()
        .nth(1)
        .map_or(word.len(), |(index, _)| index);
    for (index, c) in word.char_indices().skip(1) {
        let end = index + c.len_utf8();
        if text.width(face, px, &word[..end]) > width {
            break;
        }
        fit = end;
    }
    word.split_at(fit)
}

/// End a line with an ellipsis, taking characters off its end until the ellipsis fits.
fn ellipsize(line: &mut Line, width: i32, px: f32, text: &mut dyn Text) {
    let Some(last) = line.pieces.last_mut() else {
        return;
    };
    let face = face_of(last.style);
    loop {
        let candidate = format!("{}{ELLIPSIS}", last.text.trim_end());
        let w = text.width(face, px, &candidate);
        if last.x + w <= width || last.text.is_empty() {
            last.text = candidate;
            last.width = w;
            return;
        }
        last.text.pop();
    }
}

/// One line of `text`, cut with an ellipsis if it is wider than `width`.
pub fn fit(line: &str, width: i32, face: Face, px: f32, text: &mut dyn Text) -> String {
    if text.width(face, px, line) <= width {
        return line.to_owned();
    }
    let mut cut = line.to_owned();
    while !cut.is_empty() {
        cut.pop();
        let candidate = format!("{}{ELLIPSIS}", cut.trim_end());
        if text.width(face, px, &candidate) <= width {
            return candidate;
        }
    }
    String::new()
}

/// The stack, laid out: where each notification goes in the surface, and the surface's size.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Frame {
    pub width: i32,
    pub height: i32,
    /// One per notification shown, in the order they were given -- newest first.
    pub bubbles: Vec<Rect>,
    /// The "N more" tab, when some are waiting.
    pub more: Option<Rect>,
}

impl Frame {
    /// Stack notifications of these heights, newest first, against the corner `anchor` names.
    ///
    /// The newest goes nearest the corner. Notifications that would not fit in `max_height` are
    /// left off rather than drawn off screen; `waiting` is how many were already held back, and
    /// those left off here are added to it for the tab.
    ///
    /// Returns the frame and how many of `heights` it holds.
    pub fn new(
        heights: &[i32],
        width: i32,
        gap: i32,
        waiting: usize,
        anchor: Anchor,
        max_height: i32,
    ) -> (Self, usize) {
        let tab = |left: usize| if left > 0 { MORE + gap } else { 0 };
        let mut fitted = heights.len();
        loop {
            let used: i32 = heights[..fitted].iter().sum::<i32>()
                + gap * (fitted as i32 - 1).max(0)
                + tab(waiting + heights.len() - fitted);
            // At least one is always shown, even on a screen too short for it: a notification
            // cut off at the bottom is better than one never shown at all.
            if used <= max_height || fitted <= 1 {
                break;
            }
            fitted -= 1;
        }
        let left = waiting + heights.len() - fitted;

        // Laid out top down, then flipped for a bottom corner so the newest is at the bottom.
        let mut y = 0;
        let mut bubbles = Vec::with_capacity(fitted);
        for &height in &heights[..fitted] {
            bubbles.push(Rect::new(0, y, width, height));
            y += height + gap;
        }
        let more = (left > 0).then(|| {
            let rect = Rect::new(0, y, width, MORE);
            y += MORE + gap;
            rect
        });
        let height = (y - gap).max(0);
        let mut frame = Self {
            width,
            height,
            bubbles,
            more,
        };
        if !anchor.is_top() {
            let flip = |rect: Rect| Rect::new(rect.x, height - rect.y - rect.h, rect.w, rect.h);
            frame.bubbles = frame.bubbles.into_iter().map(flip).collect();
            frame.more = frame.more.map(flip);
        }
        (frame, fitted)
    }

    /// Which notification, by position in the list given to [`Frame::new`], is under a point.
    pub fn bubble_at(&self, x: i32, y: i32) -> Option<usize> {
        self.bubbles.iter().position(|rect| rect.contains(x, y))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Seven pixels a character, whatever the face; lines 10 tall.
    struct Fixed;

    impl Text for Fixed {
        fn width(&mut self, _face: Face, _px: f32, text: &str) -> i32 {
            text.chars().count() as i32 * 7
        }
        fn line_height(&mut self, _face: Face, _px: f32) -> i32 {
            10
        }
    }

    fn plain(text: &str) -> Vec<Span> {
        vec![Span {
            text: text.to_owned(),
            style: Style::default(),
        }]
    }

    fn texts(lines: &[Line]) -> Vec<String> {
        lines
            .iter()
            .map(|line| {
                line.pieces
                    .iter()
                    .map(|piece| piece.text.as_str())
                    .collect::<Vec<_>>()
                    .join("|")
            })
            .collect()
    }

    #[test]
    fn words_wrap_at_spaces() {
        // 10 characters a line.
        let lines = wrap(
            &plain("the quick brown fox jumps"),
            70,
            10,
            BODY_PX,
            &mut Fixed,
        );
        assert_eq!(texts(&lines), vec!["the quick", "brown fox", "jumps"]);
    }

    #[test]
    fn runs_of_space_collapse_and_hard_breaks_are_kept() {
        let lines = wrap(
            &plain("one    two\nthree\n\nfour\n"),
            700,
            10,
            BODY_PX,
            &mut Fixed,
        );
        assert_eq!(texts(&lines), vec!["one two", "three", "", "four"]);
    }

    #[test]
    fn a_word_too_long_for_a_line_is_broken_between_characters() {
        let lines = wrap(&plain("https://example.org/a"), 70, 10, BODY_PX, &mut Fixed);
        assert_eq!(texts(&lines), vec!["https://ex", "ample.org/", "a"]);
    }

    #[test]
    fn too_many_lines_end_in_an_ellipsis() {
        let lines = wrap(
            &plain("aaaa bbbb cccc dddd eeee"),
            70,
            2,
            BODY_PX,
            &mut Fixed,
        );
        assert_eq!(texts(&lines), vec!["aaaa bbbb", "cccc dddd…"]);
        assert!(lines[1].end() <= 70);
        let lines = wrap(&plain("one\ntwo\nthree"), 700, 2, BODY_PX, &mut Fixed);
        assert_eq!(texts(&lines), vec!["one", "two…"]);
    }

    #[test]
    fn styles_change_mid_line() {
        let spans = crate::markup::parse("a <b>bold</b> word");
        let lines = wrap(&spans, 700, 10, BODY_PX, &mut Fixed);
        // Spaces between pieces of different styles belong to neither; they are the gap before
        // the next piece, where an underline would not run through them.
        assert_eq!(texts(&lines), vec!["a|bold|word"]);
        let pieces = &lines[0].pieces;
        assert!(pieces[1].style.bold);
        assert_eq!(pieces[1].x, 14);
        assert_eq!(pieces[2].x, 14 + 28 + 7);
    }

    #[test]
    fn nothing_but_space_is_no_lines() {
        assert!(wrap(&plain("  \n "), 700, 10, BODY_PX, &mut Fixed).is_empty());
        assert!(wrap(&[], 700, 10, BODY_PX, &mut Fixed).is_empty());
    }

    #[test]
    fn a_title_too_long_is_cut() {
        assert_eq!(
            fit("Firefox", 700, Face::Bold, TITLE_PX, &mut Fixed),
            "Firefox"
        );
        assert_eq!(
            fit("Thunderbird", 42, Face::Bold, TITLE_PX, &mut Fixed),
            "Thund…"
        );
    }

    #[test]
    fn the_chrome_matches_a_fixed_size_4dwm_window() {
        let chrome = Chrome::new(360, 64);
        assert_eq!(chrome.frame, Rect::new(0, 0, 360, 8 + 30 + 64 + 8));
        assert_eq!(chrome.button, Rect::new(8, 8, 30, 30));
        assert_eq!(chrome.title.x, 38);
        assert_eq!(
            chrome.title.x + chrome.title.w,
            352,
            "the title runs to the border"
        );
        assert_eq!(chrome.client, Rect::new(8, 38, 344, 64));
    }

    #[test]
    fn a_bubble_is_as_tall_as_its_picture_or_its_text() {
        let body = plain("short");
        let content = Content {
            title: "App",
            summary: "Hello",
            body: &body,
            picture: Some((48, 48)),
            value: None,
        };
        let bubble = Bubble::new(&content, 360, 48, 6, &mut Fixed);
        // The picture is taller than two 10-pixel lines.
        assert_eq!(bubble.chrome.client.h, PAD + 48 + PAD);
        assert_eq!(bubble.picture, Some(Rect::new(16, 46, 48, 48)));
        assert_eq!(bubble.summary[0].pieces[0].x, 8 + 8 + 48 + 8);

        let long = plain(&"word ".repeat(200));
        let content = Content {
            body: &long,
            picture: None,
            ..content
        };
        let bubble = Bubble::new(&content, 360, 48, 6, &mut Fixed);
        assert_eq!(bubble.body.len(), 6);
        assert_eq!(bubble.chrome.client.h, PAD + 10 + PARAGRAPH + 6 * 10 + PAD);
        assert_eq!(bubble.summary[0].pieces[0].x, 8 + 8, "no picture, no slot");
    }

    #[test]
    fn a_small_picture_is_centered_in_its_slot() {
        let content = Content {
            title: "",
            summary: "x",
            body: &[],
            picture: Some((16, 32)),
            value: None,
        };
        let bubble = Bubble::new(&content, 360, 48, 6, &mut Fixed);
        assert_eq!(bubble.picture, Some(Rect::new(16 + 16, 46 + 8, 16, 32)));
    }

    #[test]
    fn a_value_adds_a_bar() {
        let content = Content {
            title: "Volume",
            summary: "",
            body: &[],
            picture: None,
            value: Some(40),
        };
        let bubble = Bubble::new(&content, 360, 48, 6, &mut Fixed);
        let bar = bubble.bar.unwrap();
        assert_eq!(
            bar.y,
            BORDER + TITLEBAR + PAD,
            "nothing above it, so no gap"
        );
        assert_eq!(bar.h, BAR);
    }

    #[test]
    fn the_newest_is_nearest_the_corner() {
        let (top, shown) = Frame::new(&[50, 60], 300, 6, 0, Anchor::TopRight, 1000);
        assert_eq!(shown, 2);
        assert_eq!(top.bubbles[0], Rect::new(0, 0, 300, 50));
        assert_eq!(top.bubbles[1], Rect::new(0, 56, 300, 60));
        assert_eq!(top.height, 116);
        assert_eq!(top.more, None);

        let (bottom, _) = Frame::new(&[50, 60], 300, 6, 0, Anchor::BottomLeft, 1000);
        assert_eq!(
            bottom.bubbles[0],
            Rect::new(0, 66, 300, 50),
            "newest at the bottom"
        );
        assert_eq!(bottom.bubbles[1], Rect::new(0, 0, 300, 60));
    }

    #[test]
    fn waiting_ones_get_a_tab_on_the_far_side() {
        let (top, _) = Frame::new(&[50], 300, 6, 3, Anchor::TopRight, 1000);
        assert_eq!(top.more, Some(Rect::new(0, 56, 300, MORE)));
        let (bottom, _) = Frame::new(&[50], 300, 6, 3, Anchor::BottomRight, 1000);
        assert_eq!(bottom.more, Some(Rect::new(0, 0, 300, MORE)));
        assert_eq!(bottom.bubbles[0].y, MORE + 6);
    }

    #[test]
    fn what_does_not_fit_on_screen_waits() {
        let (frame, shown) = Frame::new(&[100, 100, 100], 300, 0, 0, Anchor::TopRight, 250);
        // Two plus the tab is 224; three would be 300.
        assert_eq!(shown, 2);
        assert!(frame.more.is_some());
        assert!(frame.height <= 250);
        // But one is always shown.
        let (frame, shown) = Frame::new(&[400], 300, 0, 0, Anchor::TopRight, 250);
        assert_eq!(shown, 1);
        assert_eq!(frame.height, 400);
    }

    #[test]
    fn a_point_finds_its_bubble_and_a_gap_finds_none() {
        let (frame, _) = Frame::new(&[50, 60], 300, 6, 0, Anchor::TopRight, 1000);
        assert_eq!(frame.bubble_at(10, 10), Some(0));
        assert_eq!(frame.bubble_at(10, 53), None);
        assert_eq!(frame.bubble_at(10, 70), Some(1));
    }
}
