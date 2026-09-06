//! The one shared box-sizing helper for cover art's top-right corner
//! placement, used identically by PLAY, EDIT and METADATA so the three
//! modes never duplicate this layout logic.

use ratatui::layout::Rect;

/// Width of the reserved image box, in terminal cells.
pub(super) const COVER_ART_COLS: u16 = 20;
/// Height of the reserved image box, in terminal cells. Matches PLAY's
/// `details` band height and METADATA's dedicated top band; EDIT's
/// `markers` band is one row taller, leaving a single spare row below it.
pub(super) const COVER_ART_ROWS: u16 = 5;
/// Below this many remaining columns, showing the image would squeeze the
/// rest of the band's content unreadably thin — skip it instead.
const MIN_CONTENT_COLS: u16 = 30;

/// Given a mode's existing top band, returns the (possibly narrower)
/// content rect plus the reserved top-right image rect — or `None` for the
/// image half when `show` is false or the band is too narrow to spare the
/// corner box. Pure geometry: never decides *whether* an image should be
/// shown (that's the caller's job, from `App`/`Session`/CLI/toggle state),
/// only how to lay it out if it should be.
pub(super) fn split_for_cover_art(band: Rect, show: bool) -> (Rect, Option<Rect>) {
    if !show || band.width < COVER_ART_COLS + MIN_CONTENT_COLS {
        return (band, None);
    }

    let image = Rect {
        x: band.x + band.width - COVER_ART_COLS,
        y: band.y,
        width: COVER_ART_COLS,
        height: COVER_ART_ROWS.min(band.height),
    };
    let content = Rect {
        x: band.x,
        y: band.y,
        width: band.width - COVER_ART_COLS,
        height: band.height,
    };
    (content, Some(image))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn show_false_passes_the_band_through_unsplit() {
        let band = Rect::new(0, 0, 80, 5);
        let (content, image) = split_for_cover_art(band, false);
        assert_eq!(content, band);
        assert!(image.is_none());
    }

    #[test]
    fn a_wide_band_splits_correctly() {
        let band = Rect::new(0, 10, 80, 5);
        let (content, image) = split_for_cover_art(band, true);
        let image = image.expect("wide enough to fit the image");
        assert_eq!(image.width, COVER_ART_COLS);
        assert_eq!(image.height, COVER_ART_ROWS);
        assert_eq!(image.x, band.x + band.width - COVER_ART_COLS);
        assert_eq!(image.y, band.y);
        assert_eq!(content.width, band.width - COVER_ART_COLS);
        assert_eq!(content.x, band.x);
    }

    #[test]
    fn a_too_narrow_band_falls_back_to_unsplit_even_when_shown() {
        let band = Rect::new(0, 0, COVER_ART_COLS + MIN_CONTENT_COLS - 1, 5);
        let (content, image) = split_for_cover_art(band, true);
        assert_eq!(content, band);
        assert!(image.is_none());
    }

    #[test]
    fn the_exact_minimum_width_still_fits() {
        let band = Rect::new(0, 0, COVER_ART_COLS + MIN_CONTENT_COLS, 5);
        let (_, image) = split_for_cover_art(band, true);
        assert!(image.is_some());
    }

    #[test]
    fn image_height_is_clamped_to_a_shorter_band() {
        let band = Rect::new(0, 0, 80, 3);
        let (_, image) = split_for_cover_art(band, true);
        assert_eq!(image.unwrap().height, 3);
    }
}
