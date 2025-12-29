use blinksy::{
    color::{Hsv, HsvHueRainbow},
    layout::Layout2d,
    markers::Dim2d,
    pattern::Pattern,
};

/// Configuration parameters for the Rainbow pattern.
#[derive(Debug)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct WhiteBlockParams {
    pub x_min: f32,
    pub x_max: f32,
    pub y_min: f32,
    pub y_max: f32,
}

impl Default for WhiteBlockParams {
    fn default() -> Self {
        Self {
            x_min: -0.5,
            x_max: 0.5,
            y_min: -0.5,
            y_max: 0.5,
        }
    }
}

/// Rainbow pattern implementation.
///
/// Creates a smooth transition through the full HSV spectrum across the LED layout.
#[derive(Debug)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct WhiteBlock {
    /// Configuration parameters
    params: WhiteBlockParams,
}

// impl<Layout> Pattern<Dim1d, Layout> for WhiteBlock
// where
//     Layout: Layout1d,
// {
//     type Params = WhiteBlockParams;
//     type Color = Hsv<HsvHueRainbow>;
//
//     /// Creates a new Rainbow pattern with the specified parameters.
//     fn new(params: Self::Params) -> Self {
//         Self { params }
//     }
//
//     /// Generates colors for a 1D layout.
//     ///
//     /// The rainbow pattern creates a smooth transition of hues across the layout,
//     /// which shifts over time to create a flowing effect.
//     fn tick(&self, time_in_ms: u64) -> impl Iterator<Item = Self::Color> {
//         let Self { params } = self;
//         let WhiteBlockParams {
//             x_min,
//             x_max,
//             y_min,
//             y_max
//         } = params;
//
//         let time = time_in_ms as f32 * time_scalar;
//
//         Layout::points().map(move |x| {
//             let hue = x * step + time;
//             let saturation = 1.;
//             let value = 1.;
//             Self::Color::new(hue, saturation, value)
//         })
//     }
// }

impl<Layout> Pattern<Dim2d, Layout> for WhiteBlock
where
    Layout: Layout2d,
{
    type Params = WhiteBlockParams;
    type Color = Hsv<HsvHueRainbow>;

    /// Creates a new Rainbow pattern with the specified parameters.
    fn new(params: Self::Params) -> Self {
        Self { params }
    }

    /// Generates colors for a 2D layout.
    ///
    /// In 2D, the rainbow pattern uses the x-coordinate to determine hue,
    /// creating bands of color that move across the layout over time.
    fn tick(&self, time_in_ms: u64) -> impl Iterator<Item = Self::Color> {
        let Self { params } = self;
        let WhiteBlockParams {
            x_min,
            x_max,
            y_min,
            y_max,
        } = *params;

        Layout::points().map(move |point| {
            let state =
                (x_min <= point.x && point.x <= x_max) && (y_min <= point.y && point.y <= y_max);
            Self::Color::new(0.0, 0.0, if state { 1.0 } else { 0.0 })
        })
    }
}

// impl<Layout> Pattern<Dim3d, Layout> for WhiteBlock
// where
//     Layout: Layout3d,
// {
//     type Params = WhiteBlockParams;
//     type Color = Hsv<HsvHueRainbow>;
//
//     /// Creates a new Rainbow pattern with the specified parameters.
//     fn new(params: Self::Params) -> Self {
//         Self { params }
//     }
//
//     /// Generates colors for a 3D layout.
//     ///
//     /// In 3D, the rainbow pattern uses the x-coordinate to determine hue,
//     /// creating bands of color that move across the layout over time.
//     fn tick(&self, time_in_ms: u64) -> impl Iterator<Item = Self::Color> {
//         let Self { params } = self;
//         let WhiteBlockParams {
//             time_scalar,
//             position_scalar,
//         } = params;
//
//         let time = time_in_ms as f32 * time_scalar;
//         let step = 0.5 * position_scalar;
//
//         Layout::points().map(move |point| {
//             let hue = (point.x + point.y + point.z) * step + time;
//             let saturation = 1.;
//             let value = 1.;
//             Self::Color::new(hue, saturation, value)
//         })
//     }
// }
