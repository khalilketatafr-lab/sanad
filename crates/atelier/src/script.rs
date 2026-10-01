//! Script data the shaper does not expose: Arabic cursive joining and kashida
//! opportunities.
//!
//! Joining types follow Unicode `ArabicShaping.txt` (Arabic, Arabic
//! Supplement and the joiners), the only ranges the launch fonts cover.
//! Kashida priorities follow the established rules for justifying Naskh
//! (Microsoft's Arabic justification rules; W3C `alreq` §3.3). Lower numbers
//! are better places to elongate:
//!
//! | Priority | Elongate between a joined pair (a → b) when … |
//! |---|---|
//! | 1 | a is Seen, Sheen, Sad or Dad (initial or medial) |
//! | 2 | b is a final Teh Marbuta, Heh, Dal or Thal |
//! | 3 | b is a final Alef, Tah, Zah, Lam, Kaf, Keheh or Gaf (never Lam → Alef: that is a ligature) |
//! | 4 | b is a medial Beh-form followed by Reh, Zain, Yeh or Alef Maksura |
//! | 5 | b is a final Waw, Ain, Ghain, Qaf or Feh |
//! | 6 | b is any other final letter |
//!
//! The Compositor keeps at most one opportunity per word (the best one) and
//! spends kashida before widening word spaces.

/// Cursive joining type (Unicode `Joining_Type`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Joining {
    /// Non-joining.
    U,
    /// Right-joining: connects to the preceding letter only (Alef, Dal, Reh, Waw…).
    R,
    /// Dual-joining.
    D,
    /// Join-causing (Tatweel, ZWJ).
    C,
    /// Transparent (marks): skipped when deciding whether neighbors join.
    T,
}

#[must_use]
pub fn joining(c: char) -> Joining {
    use Joining::{C, D, R, T, U};
    match u32::from(c) {
        0x0610..=0x061A
        | 0x064B..=0x065F
        | 0x0670
        | 0x06D6..=0x06DC
        | 0x06DF..=0x06E4
        | 0x06E7
        | 0x06E8
        | 0x06EA..=0x06ED
        | 0x08D3..=0x08E1
        | 0x08E3..=0x08FF
        | 0x200B => T,
        // Right-joining first: it carves exceptions out of the D range 0750–077F.
        0x0622..=0x0625
        | 0x0627
        | 0x0629
        | 0x062F..=0x0632
        | 0x0648
        | 0x0671..=0x0673
        | 0x0675..=0x0677
        | 0x0688..=0x0699
        | 0x06C0
        | 0x06C3..=0x06CB
        | 0x06CD
        | 0x06CF
        | 0x06D2
        | 0x06D3
        | 0x06D5
        | 0x06EE
        | 0x06EF
        | 0x0759..=0x075B
        | 0x076B
        | 0x076C
        | 0x0771
        | 0x0773
        | 0x0774
        | 0x0778
        | 0x0779 => R,
        0x0620
        | 0x0626
        | 0x0628
        | 0x062A..=0x062E
        | 0x0633..=0x063F
        | 0x0641..=0x0647
        | 0x0649
        | 0x064A
        | 0x066E
        | 0x066F
        | 0x0678..=0x0687
        | 0x069A..=0x06BF
        | 0x06C1
        | 0x06C2
        | 0x06CC
        | 0x06CE
        | 0x06D0
        | 0x06D1
        | 0x06FA..=0x06FC
        | 0x06FF
        | 0x0750..=0x077F => D,
        0x0640 | 0x200D => C,
        _ => U,
    }
}

/// Does `c` connect to the following letter?
#[must_use]
pub fn joins_next(c: char) -> bool {
    matches!(joining(c), Joining::D | Joining::C)
}

/// Does `c` connect to the preceding letter?
#[must_use]
pub fn joins_prev(c: char) -> bool {
    matches!(joining(c), Joining::D | Joining::R | Joining::C)
}

#[must_use]
pub fn is_arabic(c: char) -> bool {
    matches!(u32::from(c), 0x0600..=0x06FF | 0x0750..=0x077F | 0x08A0..=0x08FF | 0xFB50..=0xFDFF | 0xFE70..=0xFEFF)
}

const ALEF: [char; 5] = ['\u{0627}', '\u{0622}', '\u{0623}', '\u{0625}', '\u{0671}'];
const LAM: char = '\u{0644}';
const TATWEEL: char = '\u{0640}';

/// Kashida priority for elongating the joint between `a` and `b`, where `a`
/// connects to `b`. `b_final` says `b` does not connect onward; `after_b` is
/// the next letter (marks skipped). `None`: no elongation here.
#[must_use]
pub fn kashida_priority(a: char, b: char, b_final: bool, after_b: Option<char>) -> Option<u8> {
    if !joins_next(a) || !joins_prev(b) || a == TATWEEL || b == TATWEEL {
        return None;
    }
    if a == LAM && ALEF.contains(&b) {
        return None; // Lam-Alef is a ligature: there is no joint to stretch.
    }
    if matches!(a, '\u{0633}'..='\u{0636}') {
        return Some(1);
    }
    if b_final {
        return match b {
            '\u{0629}' | '\u{0647}' | '\u{062F}' | '\u{0630}' => Some(2),
            _ if ALEF.contains(&b) => Some(3),
            '\u{0637}' | '\u{0638}' | LAM | '\u{0643}' | '\u{06A9}' | '\u{06AF}' => Some(3),
            '\u{0648}' | '\u{0639}' | '\u{063A}' | '\u{0642}' | '\u{0641}' => Some(5),
            _ => Some(6),
        };
    }
    let medial_beh = matches!(
        b,
        '\u{0628}' | '\u{062A}' | '\u{062B}' | '\u{0646}' | '\u{064A}' | '\u{0626}'
    );
    if medial_beh
        && matches!(
            after_b,
            Some('\u{0631}' | '\u{0632}' | '\u{064A}' | '\u{0649}')
        )
    {
        return Some(4);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joining_classes() {
        assert_eq!(joining('ب'), Joining::D);
        assert_eq!(joining('ا'), Joining::R);
        assert_eq!(joining('ء'), Joining::U);
        assert_eq!(joining('\u{064E}'), Joining::T, "fatha is transparent");
        assert_eq!(joining('ـ'), Joining::C);
        assert_eq!(joining('a'), Joining::U);
    }

    #[test]
    fn priorities_follow_the_naskh_rules() {
        // سلام: Seen joins Lam → priority 1.
        assert_eq!(kashida_priority('س', 'ل', false, Some('ا')), Some(1));
        // كلمة: Meem → final Teh Marbuta → 2.
        assert_eq!(kashida_priority('م', 'ة', true, None), Some(2));
        // كتاب: Teh → final Alef → 3.
        assert_eq!(kashida_priority('ت', 'ا', true, Some('ب')), Some(3));
        // Lam-Alef never.
        assert_eq!(kashida_priority('ل', 'ا', true, None), None);
        // كبير: Kaf → medial Beh followed by Yeh → 4.
        assert_eq!(kashida_priority('ك', 'ب', false, Some('ي')), Some(4));
        // Right-joining letters never connect onward.
        assert_eq!(kashida_priority('د', 'ب', false, None), None);
        // A plain medial joint is not an opportunity.
        assert_eq!(kashida_priority('م', 'ع', false, Some('ل')), None);
    }
}
