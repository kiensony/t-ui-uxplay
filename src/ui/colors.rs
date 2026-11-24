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

const SOFT_PINK: Color = Rgb(246, 187, 212);
const LIGHT_BLUE: Color = Rgb(172, 218, 245);

const FEMBOY: Colors = Colors {
    background: Rgb(28, 23, 36),
    surface: Rgb(42, 35, 51),
    border: Rgb(81, 74, 99),
    text: Rgb(252, 243, 249),
    muted: LIGHT_BLUE,
    dim: Rgb(159, 155, 178),
    accent: SOFT_PINK,
    warning: Rgb(255, 192, 150),
    spectrum_background: Rgb(25, 21, 34),
    spectrum_grid: Rgb(65, 58, 81),
    spectrum_low: LIGHT_BLUE,
    spectrum_mid: Rgb(211, 202, 233),
    spectrum_high: SOFT_PINK,
    spectrum_peak: Rgb(255, 237, 246),
    ribbons: [
        (Rgb(46, 64, 86), Rgb(75, 45, 67)),
        (Rgb(102, 145, 176), Rgb(166, 110, 141)),
        (LIGHT_BLUE, SOFT_PINK),
        (Rgb(204, 237, 255), Rgb(255, 219, 234)),
    ],
};

const DEEP_PINK: Color = Rgb(220, 88, 145);
const ROYAL_BLUE: Color = Rgb(92, 141, 221);

const TRANSGIRL: Colors = Colors {
    background: Rgb(17, 17, 33),
    surface: Rgb(24, 27, 46),
    border: Rgb(32, 36, 63),
    text: Rgb(239, 249, 255),
    muted: DEEP_PINK,
    dim: Rgb(139, 146, 175),
    accent: ROYAL_BLUE,
    warning: Rgb(255, 199, 159),
    spectrum_background: Rgb(14, 16, 32),
    spectrum_grid: Rgb(45, 44, 78),
    spectrum_low: ROYAL_BLUE,
    spectrum_mid: Rgb(159, 101, 192),
    spectrum_high: DEEP_PINK,
    spectrum_peak: Rgb(240, 140, 184),
    ribbons: [
        (Rgb(31, 42, 84), Rgb(73, 28, 57)),
        (Rgb(54, 87, 157), Rgb(148, 49, 97)),
        (ROYAL_BLUE, DEEP_PINK),
        (Rgb(126, 164, 238), Rgb(240, 140, 184)),
    ],
};
