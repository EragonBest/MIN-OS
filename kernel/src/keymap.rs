//! Layout di tastiera (scancode set 1 -> carattere).
//!
//! Ogni tabella ha 48 caratteri, nell'ordine dei tasti fisici:
//! riga numeri (0x02..0x0D), riga Q (0x10..0x1B), riga A (0x1E..0x29),
//! tasto 0x2B, riga Z (0x2C..0x35), tasto ISO 0x56.

#[derive(Clone, Copy, PartialEq)]
pub enum Layout {
    It,
    Us,
    Uk,
    De,
    Fr,
    Es,
}

pub const ALL: [Layout; 6] = [Layout::It, Layout::Us, Layout::Uk, Layout::De, Layout::Fr, Layout::Es];

impl Layout {
    pub fn code(self) -> &'static str {
        match self {
            Layout::It => "IT",
            Layout::Us => "US",
            Layout::Uk => "UK",
            Layout::De => "DE",
            Layout::Fr => "FR",
            Layout::Es => "ES",
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Layout::It => "Italiano",
            Layout::Us => "English (US)",
            Layout::Uk => "English (UK)",
            Layout::De => "Deutsch",
            Layout::Fr => "Français (AZERTY)",
            Layout::Es => "Español",
        }
    }
    pub fn sample(self) -> &'static str {
        match self {
            Layout::De => "QWERTZ",
            Layout::Fr => "AZERTY",
            _ => "QWERTY",
        }
    }

    fn tables(self) -> (&'static str, &'static str) {
        match self {
            Layout::Us => (
                "1234567890-=qwertyuiop[]asdfghjkl;'`\\zxcvbnm,./\\",
                "!@#$%^&*()_+QWERTYUIOP{}ASDFGHJKL:\"~|ZXCVBNM<>?|",
            ),
            Layout::Uk => (
                "1234567890-=qwertyuiop[]asdfghjkl;'`#zxcvbnm,./\\",
                "!\"£$%^&*()_+QWERTYUIOP{}ASDFGHJKL:@¬~ZXCVBNM<>?|",
            ),
            Layout::It => (
                "1234567890'ìqwertyuiopè+asdfghjklòà\\ùzxcvbnm,.-<",
                "!\"£$%&/()=?^QWERTYUIOPé*ASDFGHJKLç°|§ZXCVBNM;:_>",
            ),
            Layout::De => (
                "1234567890ß´qwertzuiopü+asdfghjklöä^#yxcvbnm,.-<",
                "!\"§$%&/()=?`QWERTZUIOPÜ*ASDFGHJKLÖÄ°'YXCVBNM;:_>",
            ),
            Layout::Fr => (
                "&é\"'(-è_çà)=azertyuiop^$qsdfghjklmù²*wxcvbn,;:!<",
                "1234567890°+AZERTYUIOP¨£QSDFGHJKLM%²µWXCVBN?./§>",
            ),
            Layout::Es => (
                "1234567890'¡qwertyuiop`+asdfghjklñ´ºçzxcvbnm,.-<",
                "!\"·$%&/()=?¿QWERTYUIOP^*ASDFGHJKLÑ¨ªÇZXCVBNM;:_>",
            ),
        }
    }

    fn altgr(self, sc: u8, shift: bool) -> Option<char> {
        let c = match (self, sc) {
            (Layout::It, 0x1A) => if shift { '{' } else { '[' },
            (Layout::It, 0x1B) => if shift { '}' } else { ']' },
            (Layout::It, 0x27) => '@',
            (Layout::It, 0x28) => '#',
            (Layout::It, 0x12) => '€',
            (Layout::It, 0x29) => '`',
            (Layout::It, 0x0C) => '`',
            (Layout::It, 0x0D) => '~',
            (Layout::De, 0x10) => '@',
            (Layout::De, 0x12) => '€',
            (Layout::De, 0x08) => '{',
            (Layout::De, 0x09) => '[',
            (Layout::De, 0x0A) => ']',
            (Layout::De, 0x0B) => '}',
            (Layout::De, 0x0C) => '\\',
            (Layout::De, 0x1B) => '~',
            (Layout::De, 0x56) => '|',
            (Layout::Fr, 0x03) => '~',
            (Layout::Fr, 0x04) => '#',
            (Layout::Fr, 0x05) => '{',
            (Layout::Fr, 0x06) => '[',
            (Layout::Fr, 0x07) => '|',
            (Layout::Fr, 0x08) => '`',
            (Layout::Fr, 0x09) => '\\',
            (Layout::Fr, 0x0A) => '^',
            (Layout::Fr, 0x0B) => '@',
            (Layout::Fr, 0x0C) => ']',
            (Layout::Fr, 0x0D) => '}',
            (Layout::Fr, 0x12) => '€',
            (Layout::Es, 0x02) => '|',
            (Layout::Es, 0x03) => '@',
            (Layout::Es, 0x04) => '#',
            (Layout::Es, 0x1A) => '[',
            (Layout::Es, 0x1B) => ']',
            (Layout::Es, 0x28) => '{',
            (Layout::Es, 0x2B) => '}',
            (Layout::Es, 0x29) => '\\',
            (Layout::Es, 0x12) => '€',
            (Layout::Uk, 0x12) => '€',
            (_, _) => return None,
        };
        Some(c)
    }

    pub fn map(self, sc: u8, shift: bool, altgr: bool, caps: bool) -> Option<char> {
        if sc == 0x39 {
            return Some(' ');
        }
        if altgr {
            return self.altgr(sc, shift);
        }
        let idx = match sc {
            0x02..=0x0D => sc - 0x02,
            0x10..=0x1B => sc - 0x10 + 12,
            0x1E..=0x29 => sc - 0x1E + 24,
            0x2B => 36,
            0x2C..=0x35 => sc - 0x2C + 37,
            0x56 => 47,
            _ => return None,
        } as usize;
        let (n, s) = self.tables();
        let base = n.chars().nth(idx)?;
        let mut sh = shift;
        if caps && base.is_alphabetic() {
            sh = !sh;
        }
        if sh {
            s.chars().nth(idx)
        } else {
            Some(base)
        }
    }
}
