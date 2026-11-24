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
            Self::FemboyTransgirl => &FEMBOY_TRANSGIRL,
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

const FEMBOY_TRANSGIRL: Colors = Colors {
    background: Rgb(21, 20, 34),
    surface: Rgb(34, 31, 49),
    border: Rgb(73, 69, 99),
    text: Rgb(252, 243, 249),
    muted: Rgb(165, 218, 242),
    dim: Rgb(141, 154, 182),
    accent: Rgb(245, 169, 184),
    warning: Rgb(255, 192, 150),
    spectrum_background: Rgb(21, 18, 36),
    spectrum_grid: Rgb(63, 60, 87),
    spectrum_low: Rgb(91, 206, 250),
    spectrum_mid: Rgb(212, 203, 246),
    spectrum_high: Rgb(245, 169, 184),
    spectrum_peak: Rgb(255, 237, 246),
    ribbons: [
        (Rgb(40, 62, 88), Rgb(75, 43, 69)),
        (Rgb(65, 122, 156), Rgb(150, 83, 119)),
        (Rgb(91, 206, 250), Rgb(245, 169, 184)),
        (Rgb(184, 237, 255), Rgb(255, 217, 233)),
    ],
};
