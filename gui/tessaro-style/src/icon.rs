//! The inner windows' title-bar icons, drawn rather than typed. A glyph
//! (`□`, `×`) comes from a fallback font and lands on fractional device
//! pixels at most zooms, so some of its edges are sharp and others smeared
//! half as thin. These snap every edge to a whole device pixel, from where
//! the canvas is on the window and how many device pixels a point is
//! (`pixel`: the screen's scale times the zoom).

use iced::widget::canvas::{self, Frame, Geometry, Path, Stroke};
use iced::{mouse, Color, Element, Point, Rectangle, Renderer, Size, Theme};

use crate::theme;

/// The icon's box, in points.
const SIZE: f32 = 10.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Maximize,
    Restore,
    Close,
}

pub fn view<'a, M: 'a>(icon: Icon, pixel: f32) -> Element<'a, M> {
    canvas::Canvas::new(Draw {
        icon,
        pixel: pixel.max(0.5),
        color: theme::TEXT_COLOR,
    })
    .width(SIZE)
    .height(SIZE)
    .into()
}

struct Draw {
    icon: Icon,
    pixel: f32,
    color: Color,
}

impl<M> canvas::Program<M> for Draw {
    type State = ();

    fn draw(
        &self,
        _: &(),
        renderer: &Renderer,
        _: &Theme,
        bounds: Rectangle,
        _: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        let p = self.pixel;
        // A point in the canvas, moved to the nearest device pixel edge.
        let x = |at: f32| ((bounds.x + at) * p).round() / p - bounds.x;
        let y = |at: f32| ((bounds.y + at) * p).round() / p - bounds.y;
        // A line a whole number of device pixels thick, about a point.
        let line = p.round().max(1.0) / p;
        let mut bar = |left: f32, top: f32, right: f32, bottom: f32| {
            frame.fill_rectangle(
                Point::new(left, top),
                Size::new(right - left, bottom - top),
                self.color,
            );
        };
        // Boxes are outlined with filled bars between snapped edges.
        match self.icon {
            Icon::Maximize => {
                let (l, t, r, b) = (x(1.0), y(1.0), x(9.0), y(9.0));
                bar(l, t, r, t + line);
                bar(l, b - line, r, b);
                bar(l, t, l + line, b);
                bar(r - line, t, r, b);
            }
            Icon::Restore => {
                // The front window, bottom left.
                let (l, t, r, b) = (x(1.0), y(3.0), x(7.0), y(9.0));
                bar(l, t, r, t + line);
                bar(l, b - line, r, b);
                bar(l, t, l + line, b);
                bar(r - line, t, r, b);
                // The one behind it, top right, where it shows.
                let (bl, bt, br, bb) = (x(3.0), y(1.0), x(9.0), y(7.0));
                bar(bl, bt, br, bt + line);
                bar(br - line, bt, br, bb);
                bar(bl, bt, bl + line, t);
                bar(r, bb - line, br, bb);
            }
            Icon::Close => {
                let stroke = Stroke::default()
                    .with_width(line * 1.2)
                    .with_color(self.color);
                let (l, t, r, b) = (x(1.5), y(1.5), x(8.5), y(8.5));
                frame.stroke(&Path::line(Point::new(l, t), Point::new(r, b)), stroke);
                frame.stroke(&Path::line(Point::new(r, t), Point::new(l, b)), stroke);
            }
        }
        vec![frame.into_geometry()]
    }
}
