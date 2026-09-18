//! Rendering a Spectrum screen dump, the port of `src/spectrum/screen.ts`.

pub const SCREEN_SIZE: usize = 6912;

const PALETTE: [[u8; 3]; 16] = [
    [0, 0, 0],
    [0, 0, 0xd7],
    [0xd7, 0, 0],
    [0xd7, 0, 0xd7],
    [0, 0xd7, 0],
    [0, 0xd7, 0xd7],
    [0xd7, 0xd7, 0],
    [0xd7, 0xd7, 0xd7],
    [0, 0, 0],
    [0, 0, 0xff],
    [0xff, 0, 0],
    [0xff, 0, 0xff],
    [0, 0xff, 0],
    [0, 0xff, 0xff],
    [0xff, 0xff, 0],
    [0xff, 0xff, 0xff],
];

/// Attribute used where the data ends before the attribute area: black ink on
/// white paper, as after NEW.
pub const DEFAULT_ATTR: u8 = 0x38;

#[derive(Clone, Copy, Debug, Default)]
pub struct ScreenOptions {
    pub hide_attributes: bool,
    /// true = swap ink/paper for FLASH cells
    pub flash_phase: bool,
}

/// Render a 6912-byte screen dump into RGBA pixels (256x192). Missing bitmap
/// bytes are treated as 0, missing attributes as [`DEFAULT_ATTR`], so partial
/// screens stay visible.
pub fn render_screen(data: &[u8], offset: i64, opts: ScreenOptions) -> Vec<u8> {
    let mut px = vec![0u8; 256 * 192 * 4];
    let get = |i: i64| -> u8 {
        let at = offset + i;
        if at >= 0 && (at as usize) < data.len() {
            data[at as usize]
        } else if i >= 6144 {
            DEFAULT_ATTR
        } else {
            0
        }
    };
    for y in 0..192i64 {
        let row_addr = ((y & 0xc0) << 5) | ((y & 7) << 8) | ((y & 0x38) << 2);
        for cx in 0..32i64 {
            let bits = get(row_addr + cx);
            let attr = get(6144 + (y >> 3) * 32 + cx);
            let mut ink = usize::from(attr & 7);
            let mut paper = usize::from((attr >> 3) & 7);
            let bright = if attr & 0x40 != 0 { 8 } else { 0 };
            if attr & 0x80 != 0 && opts.flash_phase {
                std::mem::swap(&mut ink, &mut paper);
            }
            let ink_c = if opts.hide_attributes { [0, 0, 0] } else { PALETTE[ink + bright] };
            let pap_c = if opts.hide_attributes { [0xff, 0xff, 0xff] } else { PALETTE[paper + bright] };
            for b in 0..8i64 {
                let on = (bits >> (7 - b)) & 1;
                let c = if on != 0 { ink_c } else { pap_c };
                let o = ((y * 256 + cx * 8 + b) * 4) as usize;
                px[o] = c[0];
                px[o + 1] = c[1];
                px[o + 2] = c[2];
                px[o + 3] = 255;
            }
        }
    }
    px
}

pub fn has_flash(data: &[u8], offset: i64) -> bool {
    for i in 6144..6912i64 {
        let at = offset + i;
        if at >= 0 {
            if let Some(v) = data.get(at as usize) {
                if v & 0x80 != 0 {
                    return true;
                }
            }
        }
    }
    false
}
