//! The faces presence detection sees, drawn over the camera panel's
//! snapshot: a box each, green while near, with its distance and, once
//! settled, its estimated age and gender. The snapshot
//! is shown contained, so the boxes are placed in the same rectangle: the
//! frame's aspect ratio fitted to the panel and centred.

use iced::widget::canvas::{self, Frame, Geometry, Path, Stroke, Text};
use iced::{mouse, Element, Length, Point, Rectangle, Renderer, Size, Theme};
use protocol::presence::FacesFrame;

use crate::theme;

pub fn view<'a, M: 'a>(faces: FacesFrame) -> Element<'a, M> {
    canvas::Canvas::new(Draw { faces })
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

struct Draw {
    faces: FacesFrame,
}

/// Where a `width` x `height` picture sits when contained in `bounds`.
fn contained(bounds: Size, width: f32, height: f32) -> Rectangle {
    if width <= 0.0 || height <= 0.0 {
        return Rectangle::new(Point::ORIGIN, bounds);
    }
    let scale = (bounds.width / width).min(bounds.height / height);
    let size = Size::new(width * scale, height * scale);
    Rectangle::new(
        Point::new(
            (bounds.width - size.width) / 2.0,
            (bounds.height - size.height) / 2.0,
        ),
        size,
    )
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
        let picture = contained(
            bounds.size(),
            self.faces.width as f32,
            self.faces.height as f32,
        );
        for face in &self.faces.faces {
            let color = if face.near {
                theme::SUCCESS
            } else {
                theme::PRIMARY
            };
            let at = Point::new(
                picture.x + face.area.x as f32 * picture.width,
                picture.y + face.area.y as f32 * picture.height,
            );
            let size = Size::new(
                face.area.w as f32 * picture.width,
                face.area.h as f32 * picture.height,
            );
            frame.stroke(
                &Path::rectangle(at, size),
                Stroke::default().with_width(2.0).with_color(color),
            );
            let mut label = format!("#{} {:.1} m", face.id, face.distance);
            if let Some(estimate) = &face.demographics {
                label.push_str("  ");
                label.push_str(&tessaro_client::describe::camera::estimate(estimate));
            }
            frame.fill_text(Text {
                content: label,
                position: Point::new(at.x, (at.y - 14.0).max(picture.y)),
                color,
                size: theme::SMALL.into(),
                ..Text::default()
            });
        }
        vec![frame.into_geometry()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wide_picture_in_a_tall_panel_sits_in_the_middle() {
        let shown = contained(Size::new(100.0, 200.0), 16.0, 9.0);
        assert_eq!(shown.width, 100.0);
        assert!((shown.height - 56.25).abs() < 1e-3);
        assert!((shown.y - (200.0 - 56.25) / 2.0).abs() < 1e-3);
        assert_eq!(shown.x, 0.0);
    }
}
