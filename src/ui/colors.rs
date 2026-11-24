use crate::settings::Palette;
use ratatui::style::Color::{self, Rgb};

pub(super) struct Colors {
    pub background: Color,
    pub surface: Color,
    pub border: Color,
    pub text: Color,
    pub muted: Color,
    pub dim: Color,
    pub accent: Color,
    pub warning: Color,
    pub spectrum_background: Color,
    pub spectrum_grid: Color,
    pub spectrum_low: Color,
    pub spectrum_mid: Color,
    pub spectrum_high: Color,
    pub spectrum_peak: Color,
    pub ribbons: [(Color, Color); 4],
}

impl Palette {
    pub(super) fn colors(self) -> &'static Colors {
        match self {
            Self::Default => &DEFAULT,
            Self::Femboy => &FEMBOY,
            Self::Transgirl => &TRANSGIRL,
        }
    }
}

const DEFAULT: Colors = Colors {
    background: Rgb(16, 22, 27),
    surface: Rgb(24, 33, 39),
    border: Rgb(53, 69, 77),
    text: Rgb(233, 239, 240),
    muted: Rgb(157, 177, 182),
    dim: Rgb(111, 129, 136),
    accent: Rgb(133, 232, 188),
    warning: Rgb(241, 190, 128),
    spectrum_background: Rgb(14, 10, 27),
    spectrum_grid: Rgb(61, 47, 86),
    spectrum_low: Rgb(89, 241, 130),
    spectrum_mid: Rgb(226, 243, 107),
    spectrum_high: Rgb(255, 169, 66),
    spectrum_peak: Rgb(255, 108, 193),
    ribbons: [
        (Rgb(42, 30, 94), Rgb(88, 19, 63)),
        (Rgb(93, 63, 204), Rgb(161, 34, 111)),
        (Rgb(109, 113, 254), Rgb(253, 73, 148)),
        (Rgb(113, 243, 255), Rgb(255, 174, 103)),
    ],
};

const FEMBOY: Colors = Colors {
    background: Rgb(28, 18, 29),
    surface: Rgb(44, 29, 43),
    border: Rgb(93, 62, 83),
    text: Rgb(252, 243, 249),
    muted: Rgb(221, 183, 205),
    dim: Rgb(172, 142, 161),
    accent: Rgb(245, 169, 184),
    warning: Rgb(255, 192, 150),
    spectrum_background: Rgb(26, 16, 29),
    spectrum_grid: Rgb(78, 49, 71),
    spectrum_low: Rgb(215, 113, 170),
    spectrum_mid: Rgb(239, 139, 188),
    spectrum_high: Rgb(245, 169, 184),
    spectrum_peak: Rgb(255, 237, 246),
    ribbons: [
        (Rgb(63, 38, 66), Rgb(75, 43, 69)),
        (Rgb(131, 76, 129), Rgb(150, 83, 119)),
        (Rgb(222, 148, 212), Rgb(245, 169, 184)),
        (Rgb(246, 204, 237), Rgb(255, 217, 233)),
    ],
};

const TRANSGIRL: Colors = Colors {
    background: Rgb(16, 23, 34),
    surface: Rgb(25, 37, 52),
    border: Rgb(51, 75, 96),
    text: Rgb(239, 249, 255),
    muted: Rgb(171, 208, 229),
    dim: Rgb(132, 158, 180),
    accent: Rgb(91, 206, 250),
    warning: Rgb(255, 199, 159),
    spectrum_background: Rgb(14, 23, 36),
    spectrum_grid: Rgb(46, 66, 89),
    spectrum_low: Rgb(91, 206, 250),
    spectrum_mid: Rgb(151, 222, 250),
    spectrum_high: Rgb(198, 236, 255),
    spectrum_peak: Rgb(255, 226, 238),
    ribbons: [
        (Rgb(32, 59, 81), Rgb(49, 57, 88)),
        (Rgb(62, 119, 153), Rgb(100, 126, 165)),
        (Rgb(91, 206, 250), Rgb(161, 205, 244)),
        (Rgb(184, 237, 255), Rgb(239, 247, 255)),
    ],
};
