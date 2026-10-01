//! UAX #9 rule L2 (reordering resolved levels) for one line.
//!
//! The server resolved embedding levels up to rule L1 when shaping (Folio
//! `Run.bidi_levels`). Line breaking happens here on the client, so L2,
//! which depends on where lines break, must happen here too. It runs per
//! line, after justification, on *clusters* rather than glyphs, so marks
//! stay attached to their base glyph.

/// Returns the visual (left-to-right) order of units given their levels:
/// `order[v]` is the logical index displayed at visual position `v`.
///
/// "From the highest level found in the text to the lowest odd level on
/// each line, reverse any contiguous sequence of characters that are at
/// that level or higher."
#[must_use]
pub fn visual_order(levels: &[u8]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..levels.len()).collect();
    let Some(&max) = levels.iter().max() else {
        return order;
    };
    let Some(lowest_odd) = levels.iter().copied().filter(|l| l % 2 == 1).min() else {
        return order; // all even: already in visual order
    };
    let mut lv = levels.to_vec();
    for level in (lowest_odd..=max).rev() {
        let mut i = 0;
        while i < lv.len() {
            if lv[i] >= level {
                let start = i;
                while i < lv.len() && lv[i] >= level {
                    i += 1;
                }
                order[start..i].reverse();
                lv[start..i].reverse();
            } else {
                i += 1;
            }
        }
    }
    order
}

#[cfg(test)]
mod tests {
    use super::visual_order;

    #[test]
    fn ltr_is_identity() {
        assert_eq!(visual_order(&[0, 0, 0]), [0, 1, 2]);
        assert_eq!(visual_order(&[]), Vec::<usize>::new());
    }

    #[test]
    fn rtl_reverses() {
        assert_eq!(visual_order(&[1, 1, 1, 1]), [3, 2, 1, 0]);
    }

    #[test]
    fn ltr_paragraph_with_rtl_phrase() {
        // "car means CAR." with CAR at level 1.
        assert_eq!(visual_order(&[0, 0, 1, 1, 1, 0]), [0, 1, 4, 3, 2, 5]);
    }

    #[test]
    fn rtl_paragraph_with_embedded_number_keeps_number_ltr() {
        // Arabic text (level 1) containing digits (level 2): the digits read
        // left-to-right inside the right-to-left line.
        assert_eq!(visual_order(&[1, 1, 2, 2, 1]), [4, 2, 3, 1, 0]);
    }

    #[test]
    fn deeper_nesting() {
        // RTL para (1) › LTR embed (2) › RTL quote (3).
        assert_eq!(visual_order(&[1, 2, 3, 3, 2, 1]), [5, 1, 3, 2, 4, 0]);
    }
}
