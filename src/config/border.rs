use std::fmt;

use pangocairo::cairo;
use serde::de;

use crate::color::Color;

/// Border color: a single color, or a list of colors drawn as a linear gradient.
pub struct Border(Vec<Color>);

impl Border {
    pub fn solid(color: Color) -> Self {
        Self(vec![color])
    }

    /// Sets the border as the source of `cr`. The gradient runs across a `width` x `height`
    /// box at `angle` degrees (0 = left to right, 90 = top to bottom), so the first and
    /// last colors land exactly on the box's extreme corners.
    pub fn apply(&self, cr: &cairo::Context, width: f64, height: f64, angle: f64) {
        let colors = match self.0.as_slice() {
            [color] => return color.apply(cr),
            colors => colors,
        };

        let (sin, cos) = angle.to_radians().sin_cos();
        let half_len = (width * cos).abs() / 2.0 + (height * sin).abs() / 2.0;
        let (cx, cy) = (width / 2.0, height / 2.0);
        let gradient = cairo::LinearGradient::new(
            cx - cos * half_len,
            cy - sin * half_len,
            cx + cos * half_len,
            cy + sin * half_len,
        );
        let last = (colors.len() - 1) as f64;
        for (i, color) in colors.iter().enumerate() {
            let (r, g, b, a) = color.rgba();
            gradient.add_color_stop_rgba(i as f64 / last, r, g, b, a);
        }
        cr.set_source(&gradient).unwrap();
    }
}

impl<'de> de::Deserialize<'de> for Border {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: de::Deserializer<'de>,
    {
        struct BorderVisitor;

        impl<'de> de::Visitor<'de> for BorderVisitor {
            type Value = Border;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a color or a list of colors")
            }

            fn visit_str<E>(self, s: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                de::Deserialize::deserialize(de::value::StrDeserializer::new(s)).map(Border::solid)
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: de::SeqAccess<'de>,
            {
                let mut colors = Vec::new();
                while let Some(color) = seq.next_element()? {
                    colors.push(color);
                }
                if colors.is_empty() {
                    return Err(de::Error::custom("border color list cannot be empty"));
                }
                Ok(Border(colors))
            }
        }

        deserializer.deserialize_any(BorderVisitor)
    }
}
