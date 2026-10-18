// SPDX-License-Identifier: GPL-3.0-or-later
//! Drawing the stack.
//!
//! Everything here takes geometry that has already been decided ([`crate::layout`]) and pictures
//! that have already been decoded ([`crate::pixmap`]), and puts pixels in a buffer. It holds no
//! state and asks the bus nothing.
//!
//! # A notification is drawn the way the compositor draws a fixed-size window
//!
//! Same pieces, same order, same palette roles: a beveled ring for the border, inside a 1-pixel
//! keyline; a titlebar of two raised pieces, the window-menu button with its bar glyph and the
//! title piece; then the body. The compositor builds GPU quads and this fills pixels, but both
//! get their geometry from `wlrix_ui::bevel`, so they agree to the pixel.
//!
//! # Active and inactive
//!
//! A window's frame is gold when it has focus and gray when it does not. A notification takes
//! the active colors in two cases: while the pointer is over it, which is as close as it comes
//! to having focus, and always when it is critical -- the one case where it should look as if it
//! wants attention.
//!
//! # The surface starts transparent
//!
//! The gaps between notifications belong to whatever is underneath. They are left clear, and
//! the input region leaves them out too, so a click there reaches the window below.

use wlrix_ui::bevel::{self, Run, Shade};
use wlrix_ui::canvas::{Canvas, Rect};
use wlrix_ui::color::Rgb;
use wlrix_ui::motif::{self, Bevel};
use wlrix_ui::palette::Palette;
use wlrix_ui::text::{Face, Fonts, Run as TextRun};

use crate::layout::{
    BEVEL, BODY_PX, BORDER, Bubble, Frame, Line, OUTLINE, SUMMARY_PX, TITLE_PX, face_of,
};
use crate::pixmap::Pixmap;

/// The "N more" tab's label size.
const MORE_PX: f32 = 13.0;

/// One notification on screen, with everything it needs already chosen.
pub struct Shown<'a> {
    pub bubble: &'a Bubble,
    pub picture: Option<&'a Pixmap>,
    /// Drawn in the active colors -- hovered, or critical.
    pub active: bool,
    /// The window-menu button is held down.
    pub pressed: bool,
    pub value: Option<i32>,
}

/// Everything one frame needs.
pub struct Scene<'a> {
    pub palette: &'a Palette,
    pub fonts: &'a mut Fonts,
    pub frame: &'a Frame,
    /// In the frame's order: one per rect in `frame.bubbles`.
    pub shown: &'a [Shown<'a>],
    /// How many are waiting, for the tab.
    pub waiting: usize,
}

/// Paint the whole surface.
pub fn stack(canvas: &mut Canvas, scene: &mut Scene) {
    canvas.clear_transparent();
    for (rect, shown) in scene.frame.bubbles.iter().zip(scene.shown) {
        bubble(canvas, scene.palette, scene.fonts, *rect, shown);
    }
    if let Some(rect) = scene.frame.more {
        more(canvas, scene.palette, scene.fonts, rect, scene.waiting);
    }
}

/// A frame state's tones, as the compositor's `frame_palette` picks them.
struct Tones {
    face: Rgb,
    light: Rgb,
    dark: Rgb,
    armed: Rgb,
    text: Rgb,
}

fn tones(palette: &Palette, active: bool) -> Tones {
    if active {
        Tones {
            face: palette.title_active,
            light: palette.title_active_top_shadow,
            dark: palette.title_active_bottom_shadow,
            armed: palette.title_active_armed,
            text: palette.title_active_text,
        }
    } else {
        Tones {
            face: palette.title_inactive,
            light: palette.title_inactive_top_shadow,
            dark: palette.title_inactive_bottom_shadow,
            armed: palette.title_inactive_armed,
            text: palette.title_inactive_text,
        }
    }
}

/// `rect`, moved from a notification's own coordinates to the surface's.
fn at(rect: Rect, origin: Rect) -> Rect {
    Rect::new(rect.x + origin.x, rect.y + origin.y, rect.w, rect.h)
}

/// Fill a run of bevel quads in the three tones they name.
fn fill(canvas: &mut Canvas, quads: &[(Rect, Shade)], face: Rgb, light: Rgb, dark: Rgb) {
    for (rect, shade) in quads {
        let color = match shade {
            Shade::Face => face,
            Shade::Light => light,
            Shade::Dark => dark,
        };
        canvas.fill_rect(*rect, color);
    }
}

/// One raised (or, pressed, sunken) titlebar piece. `Run::Vertical`, as the compositor cuts a
/// titlebar row.
fn piece(canvas: &mut Canvas, rect: Rect, tones: &Tones, pressed: bool) {
    let mut quads = Vec::new();
    bevel::quads(&mut quads, rect, BEVEL, !pressed, Run::Vertical);
    let face = if pressed { tones.armed } else { tones.face };
    fill(canvas, &quads, face, tones.light, tones.dark);
}

/// An IRIX button glyph: a 1-pixel outline with a solid drop shadow `shadow` pixels down and
/// right, drawn only where it falls outside the outline. The compositor's `glyph_outline`.
fn glyph(canvas: &mut Canvas, rect: Rect, shadow: i32, color: Rgb) {
    canvas.stroke_rect(rect, color);
    let Rect { x, y, w, h } = rect;
    canvas.fill_rect(Rect::new(x + w, y + shadow, shadow, h), color);
    canvas.fill_rect(Rect::new(x + shadow, y + h, w - shadow, shadow), color);
}

fn bubble(canvas: &mut Canvas, palette: &Palette, fonts: &mut Fonts, origin: Rect, shown: &Shown) {
    let chrome = &shown.bubble.chrome;
    let tones = tones(palette, shown.active);
    let frame = at(chrome.frame, origin);

    // The border: an unbroken ring, a fixed-size window's, inside the keyline.
    let mut quads = Vec::new();
    bevel::ring_quads(&mut quads, frame.inset(OUTLINE), BORDER - OUTLINE, BEVEL);
    fill(canvas, &quads, tones.face, tones.light, tones.dark);
    canvas.stroke_rect(frame, palette.outer_line);

    // The titlebar: the window-menu button, which closes, and the title.
    let button = at(chrome.button, origin);
    piece(canvas, button, &tones, shown.pressed);
    // Measured off the original 30x30 IRIX button: the bar at (3, 12), 22x5, a 2-pixel shadow.
    // Pressed, the glyph moves with the face, as a sunken button's contents do.
    let nudge = i32::from(shown.pressed);
    glyph(
        canvas,
        Rect::new(button.x + 3 + nudge, button.y + 12 + nudge, 22, 5),
        2,
        palette.outer_line,
    );
    piece(canvas, at(chrome.title, origin), &tones, false);
    let title_area = at(chrome.title_text, origin);
    let baseline = fonts.centered_baseline(Face::Bold, TITLE_PX, title_area);
    fonts.draw(
        canvas,
        TextRun {
            face: Face::Bold,
            px: TITLE_PX,
            x: title_area.x,
            baseline,
            color: tones.text,
        },
        &shown.bubble.title,
    );

    // The body: a raised Motif panel, as a dialog's is.
    let client = at(chrome.client, origin);
    motif::panel(
        canvas,
        client,
        palette.panel,
        Bevel::raised(
            palette.panel_top_shadow,
            palette.panel_bottom_shadow,
            palette.metrics.shadow_thickness_default.clamp(1, 2),
        ),
    );

    if let (Some(rect), Some(picture)) = (shown.bubble.picture, shown.picture) {
        let rect = at(rect, origin);
        picture.draw(canvas, rect.x, rect.y);
    }

    let summary_ascent = fonts.ascent(Face::Bold, SUMMARY_PX);
    for line in &shown.bubble.summary {
        text_line(
            canvas,
            palette,
            fonts,
            origin,
            line,
            SUMMARY_PX,
            summary_ascent,
        );
    }
    let body_ascent = fonts.ascent(Face::Regular, BODY_PX);
    for line in &shown.bubble.body {
        text_line(canvas, palette, fonts, origin, line, BODY_PX, body_ascent);
    }

    if let (Some(rect), Some(value)) = (shown.bubble.bar, shown.value) {
        bar(canvas, palette, at(rect, origin), value);
    }
}

/// One line of text, piece by piece. Every piece in a line shares a baseline, taken from the
/// line's main face, so a bold word does not sit higher than the plain ones around it.
fn text_line(
    canvas: &mut Canvas,
    palette: &Palette,
    fonts: &mut Fonts,
    origin: Rect,
    line: &Line,
    px: f32,
    ascent: i32,
) {
    let baseline = origin.y + line.top + ascent;
    for piece in &line.pieces {
        let color = if piece.style.link {
            palette.link
        } else {
            palette.foreground
        };
        let x = origin.x + piece.x;
        fonts.draw(
            canvas,
            TextRun {
                face: face_of(piece.style),
                px,
                x,
                baseline,
                color,
            },
            &piece.text,
        );
        if piece.style.underline || piece.style.link {
            canvas.fill_rect(Rect::new(x, baseline + 2, piece.width, 1), color);
        }
    }
}

/// A progress bar: a sunken trough, and a raised slab across the part that is done -- a Motif
/// scale's look, which is what IRIX used for a level.
fn bar(canvas: &mut Canvas, palette: &Palette, rect: Rect, value: i32) {
    let trough = Bevel::sunken(palette.trough_top_shadow, palette.trough_bottom_shadow, 2);
    motif::panel(canvas, rect, palette.trough, trough);
    let inner = rect.inset(2);
    let done = inner.w * value.clamp(0, 100) / 100;
    if done >= 4 {
        motif::panel(
            canvas,
            Rect::new(inner.x, inner.y, done, inner.h),
            palette.face,
            Bevel::raised(palette.face_top_shadow, palette.face_bottom_shadow, 2),
        );
    }
}

/// The tab saying how many more are waiting: a small inactive titlebar piece with a count.
fn more(canvas: &mut Canvas, palette: &Palette, fonts: &mut Fonts, rect: Rect, waiting: usize) {
    let tones = tones(palette, false);
    piece(canvas, rect.inset(OUTLINE), &tones, false);
    canvas.stroke_rect(rect, palette.outer_line);
    let label = if waiting == 1 {
        "1 more notification".to_owned()
    } else {
        format!("{waiting} more notifications")
    };
    let width = fonts.width(Face::Bold, MORE_PX, &label);
    let baseline = fonts.centered_baseline(Face::Bold, MORE_PX, rect);
    fonts.draw(
        canvas,
        TextRun {
            face: Face::Bold,
            px: MORE_PX,
            x: rect.x + (rect.w - width).max(0) / 2,
            baseline,
            color: tones.text,
        },
        &label,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{Anchor, Content, Text};

    struct Measured<'a>(&'a mut Fonts);

    impl Text for Measured<'_> {
        fn width(&mut self, face: Face, px: f32, text: &str) -> i32 {
            self.0.width(face, px, text)
        }
        fn line_height(&mut self, face: Face, px: f32) -> i32 {
            self.0.line_height(face, px)
        }
    }

    fn pixel(buffer: &[u8], width: i32, x: i32, y: i32) -> u32 {
        let at = ((y * width + x) * 4) as usize;
        u32::from_ne_bytes(buffer[at..at + 4].try_into().unwrap())
    }

    #[test]
    fn a_notification_is_framed_and_the_gap_stays_clear() {
        let Ok(mut fonts) = Fonts::load() else {
            // No fonts at all on the build machine; the geometry is tested in `layout`.
            return;
        };
        let palette = wlrix_ui::palette::DEFAULT;
        let body = crate::markup::parse("Some <b>body</b> text");
        let content = Content {
            title: "Test",
            summary: "Hello",
            body: &body,
            picture: None,
            value: Some(50),
        };
        let bubble = Bubble::new(&content, 300, 48, 6, &mut Measured(&mut fonts));
        let heights = [bubble.height(), bubble.height()];
        let (frame, _) = Frame::new(&heights, 300, 6, 2, Anchor::TopRight, 2000);
        let shown = [
            Shown {
                bubble: &bubble,
                picture: None,
                active: true,
                pressed: false,
                value: Some(50),
            },
            Shown {
                bubble: &bubble,
                picture: None,
                active: false,
                pressed: true,
                value: Some(50),
            },
        ];
        let (width, height) = (frame.width, frame.height);
        let mut buffer = vec![0u8; (width * height * 4) as usize];
        let mut canvas = Canvas::new(&mut buffer, width, height);
        stack(
            &mut canvas,
            &mut Scene {
                palette,
                fonts: &mut fonts,
                frame: &frame,
                shown: &shown,
                waiting: 2,
            },
        );

        // The keyline at the corner, and the active face just inside the bevel.
        assert_eq!(pixel(&buffer, width, 0, 0), palette.outer_line.0);
        assert_eq!(pixel(&buffer, width, 4, 50), palette.title_active.0);
        // The gap between the two is untouched.
        let gap = frame.bubbles[0].h + 3;
        assert_eq!(pixel(&buffer, width, 150, gap), 0);
        // The second one is inactive.
        let second = frame.bubbles[1];
        assert_eq!(
            pixel(&buffer, width, second.x + 4, second.y + 50),
            palette.title_inactive.0
        );
    }
}
