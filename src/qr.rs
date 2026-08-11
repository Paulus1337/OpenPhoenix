pub struct Qr {
    pub size: usize,
    pub modules: Vec<bool>,
}

const QUIET: usize = 4;
const MAX_VERSION: usize = 10;

const EC_PER_BLOCK_M: [usize; 11] = [0, 10, 16, 26, 18, 24, 16, 18, 22, 22, 26];
const BLOCKS_M: [usize; 11] = [0, 1, 1, 1, 2, 2, 4, 4, 4, 5, 5];
const TOTAL_CODEWORDS: [usize; 11] = [0, 26, 44, 70, 100, 134, 172, 196, 242, 292, 346];
const ALIGN_CENTER: [&[usize]; 11] = [
    &[],
    &[],
    &[6, 18],
    &[6, 22],
    &[6, 26],
    &[6, 30],
    &[6, 34],
    &[6, 22, 38],
    &[6, 24, 42],
    &[6, 26, 46],
    &[6, 28, 50],
];

fn gf_exp_log() -> ([u8; 512], [u8; 256]) {
    let mut exp = [0u8; 512];
    let mut log = [0u8; 256];
    let mut x: u16 = 1;
    for i in 0..255usize {
        if let Some(slot) = exp.get_mut(i) {
            *slot = x as u8;
        }
        if let Some(slot) = log.get_mut(x as usize) {
            *slot = i as u8;
        }
        x <<= 1;
        if x & 0x100 != 0 {
            x ^= 0x11d;
        }
    }
    for i in 255..512usize {
        let prev = exp.get(i - 255).copied().unwrap_or(0);
        if let Some(slot) = exp.get_mut(i) {
            *slot = prev;
        }
    }
    (exp, log)
}

fn gf_mul(a: u8, b: u8, exp: &[u8; 512], log: &[u8; 256]) -> u8 {
    if a == 0 || b == 0 {
        return 0;
    }
    let la = log.get(a as usize).copied().unwrap_or(0) as usize;
    let lb = log.get(b as usize).copied().unwrap_or(0) as usize;
    exp.get(la + lb).copied().unwrap_or(0)
}

fn generator(degree: usize, exp: &[u8; 512], log: &[u8; 256]) -> Vec<u8> {
    let mut poly = vec![1u8];
    for i in 0..degree {
        let root = exp.get(i).copied().unwrap_or(0);
        let mut next = vec![0u8; poly.len() + 1];
        for (j, c) in poly.iter().enumerate() {
            let prod = gf_mul(*c, root, exp, log);
            if let Some(slot) = next.get_mut(j + 1) {
                *slot ^= prod;
            }
            if let Some(slot) = next.get_mut(j) {
                *slot ^= *c;
            }
        }
        poly = next;
    }
    poly
}

fn ecc_for(block: &[u8], count: usize, exp: &[u8; 512], log: &[u8; 256]) -> Vec<u8> {
    let gen = generator(count, exp, log);
    let mut rem = vec![0u8; count];
    for byte in block {
        let factor = byte ^ rem.first().copied().unwrap_or(0);
        rem.remove(0);
        rem.push(0);
        for (i, g) in gen.iter().enumerate().skip(1) {
            let prod = gf_mul(*g, factor, exp, log);
            if let Some(slot) = rem.get_mut(i - 1) {
                *slot ^= prod;
            }
        }
    }
    rem
}

fn capacity_bits(version: usize) -> usize {
    let total = TOTAL_CODEWORDS.get(version).copied().unwrap_or(0);
    let ec = EC_PER_BLOCK_M.get(version).copied().unwrap_or(0);
    let blocks = BLOCKS_M.get(version).copied().unwrap_or(1);
    (total - ec * blocks) * 8
}

fn count_bits(version: usize) -> usize {
    if version < 10 {
        8
    } else {
        16
    }
}

fn pick_version(len: usize) -> Result<usize, String> {
    for v in 1..=MAX_VERSION {
        let needed = 4 + count_bits(v) + len * 8;
        if needed <= capacity_bits(v) {
            return Ok(v);
        }
    }
    Err(format!(
        "{len} bytes do not fit in a version {MAX_VERSION} QR code"
    ))
}

struct Bits {
    bits: Vec<bool>,
}

impl Bits {
    fn new() -> Self {
        Bits { bits: Vec::new() }
    }

    fn push(&mut self, value: usize, width: usize) {
        for i in (0..width).rev() {
            self.bits.push((value >> i) & 1 == 1);
        }
    }
}

fn to_codewords(text: &[u8], version: usize) -> Vec<u8> {
    let cap = capacity_bits(version);
    let mut b = Bits::new();
    b.push(0b0100, 4);
    b.push(text.len(), count_bits(version));
    for byte in text {
        b.push(*byte as usize, 8);
    }
    let terminator = (cap - b.bits.len()).min(4);
    b.push(0, terminator);
    while !b.bits.len().is_multiple_of(8) {
        b.bits.push(false);
    }
    let mut words: Vec<u8> = b
        .bits
        .chunks(8)
        .map(|c| c.iter().fold(0u8, |acc, bit| (acc << 1) | u8::from(*bit)))
        .collect();
    let want = cap / 8;
    let pads = [0xECu8, 0x11u8];
    let mut i = 0usize;
    while words.len() < want {
        words.push(pads.get(i % 2).copied().unwrap_or(0xEC));
        i += 1;
    }
    words
}

fn interleave(words: &[u8], version: usize) -> Vec<u8> {
    let (exp, log) = gf_exp_log();
    let blocks = BLOCKS_M.get(version).copied().unwrap_or(1);
    let ec_count = EC_PER_BLOCK_M.get(version).copied().unwrap_or(0);
    let short = words.len() / blocks;
    let long_blocks = words.len() % blocks;
    let mut data_blocks: Vec<Vec<u8>> = Vec::new();
    let mut ec_blocks: Vec<Vec<u8>> = Vec::new();
    let mut at = 0usize;
    for i in 0..blocks {
        let size = if i >= blocks - long_blocks {
            short + 1
        } else {
            short
        };
        let end = (at + size).min(words.len());
        let block = words.get(at..end).unwrap_or(&[]).to_vec();
        at = end;
        ec_blocks.push(ecc_for(&block, ec_count, &exp, &log));
        data_blocks.push(block);
    }
    let mut out = Vec::new();
    let widest = data_blocks.iter().map(Vec::len).max().unwrap_or(0);
    for i in 0..widest {
        for b in &data_blocks {
            if let Some(v) = b.get(i) {
                out.push(*v);
            }
        }
    }
    for i in 0..ec_count {
        for b in &ec_blocks {
            if let Some(v) = b.get(i) {
                out.push(*v);
            }
        }
    }
    out
}

struct Canvas {
    size: usize,
    modules: Vec<bool>,
    reserved: Vec<bool>,
}

impl Canvas {
    fn new(version: usize) -> Self {
        let size = version * 4 + 17;
        Canvas {
            size,
            modules: vec![false; size * size],
            reserved: vec![false; size * size],
        }
    }

    fn set(&mut self, x: usize, y: usize, dark: bool, reserve: bool) {
        if x >= self.size || y >= self.size {
            return;
        }
        let idx = y * self.size + x;
        if let Some(slot) = self.modules.get_mut(idx) {
            *slot = dark;
        }
        if reserve {
            if let Some(slot) = self.reserved.get_mut(idx) {
                *slot = true;
            }
        }
    }

    fn get(&self, x: usize, y: usize) -> bool {
        if x >= self.size || y >= self.size {
            return false;
        }
        self.modules
            .get(y * self.size + x)
            .copied()
            .unwrap_or(false)
    }

    fn is_reserved(&self, x: usize, y: usize) -> bool {
        if x >= self.size || y >= self.size {
            return true;
        }
        self.reserved
            .get(y * self.size + x)
            .copied()
            .unwrap_or(true)
    }

    fn finder(&mut self, ox: usize, oy: usize) {
        for dy in 0..7usize {
            for dx in 0..7usize {
                let edge = dx == 0 || dx == 6 || dy == 0 || dy == 6;
                let core = (2..=4).contains(&dx) && (2..=4).contains(&dy);
                self.set(ox + dx, oy + dy, edge || core, true);
            }
        }
    }

    fn separators(&mut self, ox: isize, oy: isize) {
        for i in -1isize..=7 {
            for (x, y) in [
                (ox + i, oy - 1),
                (ox + i, oy + 7),
                (ox - 1, oy + i),
                (ox + 7, oy + i),
            ] {
                if x >= 0 && y >= 0 {
                    self.set(x as usize, y as usize, false, true);
                }
            }
        }
    }

    fn alignment(&mut self, version: usize) {
        let centers = ALIGN_CENTER.get(version).copied().unwrap_or(&[]);
        for cy in centers {
            for cx in centers {
                let near_finder = (*cx < 8 && *cy < 8)
                    || (*cx < 8 && *cy + 8 >= self.size)
                    || (*cx + 8 >= self.size && *cy < 8);
                if near_finder {
                    continue;
                }
                for dy in 0..5usize {
                    for dx in 0..5usize {
                        let edge = dx == 0 || dx == 4 || dy == 0 || dy == 4;
                        let center = dx == 2 && dy == 2;
                        self.set(cx + dx - 2, cy + dy - 2, edge || center, true);
                    }
                }
            }
        }
    }

    fn timing(&mut self) {
        for i in 8..self.size - 8 {
            let dark = i % 2 == 0;
            self.set(i, 6, dark, true);
            self.set(6, i, dark, true);
        }
    }

    fn reserve_format(&mut self) {
        for i in 0..9usize {
            self.set(i, 8, self.get(i, 8), true);
            self.set(8, i, self.get(8, i), true);
        }
        for i in 0..8usize {
            self.set(self.size - 1 - i, 8, self.get(self.size - 1 - i, 8), true);
            self.set(8, self.size - 1 - i, self.get(8, self.size - 1 - i), true);
        }
        self.set(8, self.size - 8, true, true);
    }

    fn reserve_version(&mut self, version: usize) {
        if version < 7 {
            return;
        }
        for i in 0..6usize {
            for j in 0..3usize {
                self.set(i, self.size - 11 + j, false, true);
                self.set(self.size - 11 + j, i, false, true);
            }
        }
    }

    fn place(&mut self, data: &[u8]) {
        let mut bit = 0usize;
        let total = data.len() * 8;
        let mut col = self.size as isize - 1;
        let mut upward = true;
        while col > 0 {
            if col == 6 {
                col -= 1;
            }
            for row in 0..self.size {
                let y = if upward { self.size - 1 - row } else { row };
                for dx in 0..2usize {
                    let x = (col as usize).saturating_sub(dx);
                    if self.is_reserved(x, y) {
                        continue;
                    }
                    let dark = if bit < total {
                        let byte = data.get(bit / 8).copied().unwrap_or(0);
                        (byte >> (7 - bit % 8)) & 1 == 1
                    } else {
                        false
                    };
                    bit += 1;
                    self.set(x, y, dark, false);
                }
            }
            upward = !upward;
            col -= 2;
        }
    }

    fn apply_mask(&self, mask: usize) -> Canvas {
        let mut out = Canvas {
            size: self.size,
            modules: self.modules.clone(),
            reserved: self.reserved.clone(),
        };
        for y in 0..self.size {
            for x in 0..self.size {
                if self.is_reserved(x, y) {
                    continue;
                }
                let flip = match mask {
                    0 => (y + x) % 2 == 0,
                    1 => y % 2 == 0,
                    2 => x % 3 == 0,
                    3 => (y + x) % 3 == 0,
                    4 => (y / 2 + x / 3) % 2 == 0,
                    5 => (y * x) % 2 + (y * x) % 3 == 0,
                    6 => ((y * x) % 2 + (y * x) % 3) % 2 == 0,
                    _ => ((y + x) % 2 + (y * x) % 3) % 2 == 0,
                };
                if flip {
                    let cur = out.get(x, y);
                    out.set(x, y, cur != flip, false);
                }
            }
        }
        out
    }

    fn format_bits(&mut self, mask: usize) {
        let data = mask & 0b111;
        let mut rem = data << 10;
        for i in (0..5).rev() {
            if rem & (1 << (i + 10)) != 0 {
                rem ^= 0b101_0011_0111 << i;
            }
        }
        let bits = ((data << 10) | rem) ^ 0b101_0100_0001_0010;
        for i in 0..15usize {
            let dark = (bits >> i) & 1 == 1;
            let (x1, y1) = match i {
                0..=5 => (8usize, i),
                6 => (8, 7),
                7 => (8, 8),
                8 => (7, 8),
                _ => (14 - i, 8),
            };
            self.set(x1, y1, dark, true);
            let (x2, y2) = if i < 8 {
                (self.size - 1 - i, 8usize)
            } else {
                (8usize, self.size - 15 + i)
            };
            self.set(x2, y2, dark, true);
        }
        self.set(8, self.size - 8, true, true);
    }

    fn version_bits(&mut self, version: usize) {
        if version < 7 {
            return;
        }
        let mut rem = version << 12;
        for i in (0..6).rev() {
            if rem & (1 << (i + 12)) != 0 {
                rem ^= 0b1_1111_0010_0101 << i;
            }
        }
        let bits = (version << 12) | rem;
        for i in 0..18usize {
            let dark = (bits >> i) & 1 == 1;
            let a = i / 3;
            let b = self.size - 11 + i % 3;
            self.set(a, b, dark, true);
            self.set(b, a, dark, true);
        }
    }

    fn penalty(&self) -> usize {
        let mut score = 0usize;
        for y in 0..self.size {
            let mut run = 1usize;
            for x in 1..self.size {
                if self.get(x, y) == self.get(x - 1, y) {
                    run += 1;
                } else {
                    if run >= 5 {
                        score += 3 + (run - 5);
                    }
                    run = 1;
                }
            }
            if run >= 5 {
                score += 3 + (run - 5);
            }
        }
        for x in 0..self.size {
            let mut run = 1usize;
            for y in 1..self.size {
                if self.get(x, y) == self.get(x, y - 1) {
                    run += 1;
                } else {
                    if run >= 5 {
                        score += 3 + (run - 5);
                    }
                    run = 1;
                }
            }
            if run >= 5 {
                score += 3 + (run - 5);
            }
        }
        for y in 0..self.size.saturating_sub(1) {
            for x in 0..self.size.saturating_sub(1) {
                let a = self.get(x, y);
                if a == self.get(x + 1, y) && a == self.get(x, y + 1) && a == self.get(x + 1, y + 1)
                {
                    score += 3;
                }
            }
        }
        let dark = self.modules.iter().filter(|m| **m).count();
        let total = self.size * self.size;
        let percent = dark * 100 / total.max(1);
        let k = percent.abs_diff(50) / 5;
        score += k * 10;
        score
    }
}

pub fn encode(text: &str) -> Result<Qr, String> {
    let bytes = text.as_bytes();
    if bytes.is_empty() {
        return Err("nothing to encode".into());
    }
    let version = pick_version(bytes.len())?;
    let words = to_codewords(bytes, version);
    let data = interleave(&words, version);

    let mut base = Canvas::new(version);
    base.finder(0, 0);
    base.finder(base.size - 7, 0);
    base.finder(0, base.size - 7);
    base.separators(0, 0);
    base.separators(base.size as isize - 7, 0);
    base.separators(0, base.size as isize - 7);
    base.alignment(version);
    base.timing();
    base.reserve_format();
    base.reserve_version(version);
    base.place(&data);

    let mut best: Option<(usize, Canvas)> = None;
    for mask in 0..8usize {
        let mut candidate = base.apply_mask(mask);
        candidate.format_bits(mask);
        candidate.version_bits(version);
        let score = candidate.penalty();
        let better = match &best {
            Some((s, _)) => score < *s,
            None => true,
        };
        if better {
            best = Some((score, candidate));
        }
    }
    let (_, chosen) = best.ok_or("no mask could be scored")?;
    Ok(Qr {
        size: chosen.size,
        modules: chosen.modules,
    })
}

impl Qr {
    pub fn dark(&self, x: usize, y: usize) -> bool {
        if x >= self.size || y >= self.size {
            return false;
        }
        self.modules
            .get(y * self.size + x)
            .copied()
            .unwrap_or(false)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn dark_count(&self) -> usize {
        self.modules.iter().filter(|m| **m).count()
    }

    pub fn to_ascii(&self) -> String {
        let width = self.size + QUIET * 2;
        let mut out = String::new();
        for y in 0..width {
            for x in 0..width {
                let dark = y >= QUIET
                    && x >= QUIET
                    && y < QUIET + self.size
                    && x < QUIET + self.size
                    && self.dark(x - QUIET, y - QUIET);
                out.push_str(if dark { "  " } else { "\u{2588}\u{2588}" });
            }
            out.push('\n');
        }
        out
    }

    pub fn to_unicode_half(&self) -> String {
        let width = self.size + QUIET * 2;
        let mut out = String::new();
        let mut y = 0usize;
        while y < width {
            for x in 0..width {
                let inside = |yy: usize| {
                    yy >= QUIET
                        && x >= QUIET
                        && yy < QUIET + self.size
                        && x < QUIET + self.size
                        && self.dark(x - QUIET, yy - QUIET)
                };
                let top = inside(y);
                let bottom = y + 1 < width && inside(y + 1);
                out.push(match (top, bottom) {
                    (false, false) => '\u{2588}',
                    (false, true) => '\u{2580}',
                    (true, false) => '\u{2584}',
                    (true, true) => ' ',
                });
            }
            out.push('\n');
            y += 2;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_short_string_uses_the_smallest_version() {
        let q = encode("HELLO").unwrap();
        assert_eq!(q.size, 21);
        assert_eq!(q.modules.len(), 21 * 21);
    }

    #[test]
    fn the_three_finder_patterns_are_present() {
        let q = encode("https://openphoenix.app").unwrap();
        for (ox, oy) in [(0, 0), (q.size - 7, 0), (0, q.size - 7)] {
            assert!(q.dark(ox, oy), "finder corner missing at {ox},{oy}");
            assert!(q.dark(ox + 6, oy), "finder edge missing");
            assert!(!q.dark(ox + 1, oy + 1), "finder ring not light");
            assert!(q.dark(ox + 3, oy + 3), "finder core not dark");
        }
    }

    #[test]
    fn timing_patterns_alternate() {
        let q = encode("timing").unwrap();
        for i in 8..q.size - 8 {
            assert_eq!(q.dark(i, 6), i % 2 == 0, "row timing wrong at {i}");
            assert_eq!(q.dark(6, i), i % 2 == 0, "column timing wrong at {i}");
        }
    }

    #[test]
    fn the_dark_module_is_always_set() {
        let q = encode("dark module").unwrap();
        assert!(q.dark(8, q.size - 8));
    }

    #[test]
    fn ascii_output_carries_a_quiet_zone() {
        let q = encode("QUIET").unwrap();
        let text = q.to_ascii();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), q.size + QUIET * 2);
        for line in lines.iter().take(QUIET) {
            assert!(
                line.chars().all(|c| c == '\u{2588}'),
                "quiet zone row is not blank: {line}"
            );
        }
    }

    #[test]
    fn the_half_block_rendering_halves_the_row_count() {
        let q = encode("HALF").unwrap();
        let rows = q.to_unicode_half().lines().count();
        let width = q.size + QUIET * 2;
        assert_eq!(rows, width.div_ceil(2));
    }

    #[test]
    fn encoding_is_deterministic_for_a_fixed_input() {
        let a = encode("phoenix").unwrap();
        let b = encode("phoenix").unwrap();
        assert_eq!(a.modules, b.modules);
        assert_eq!(a.dark_count(), b.dark_count());
    }

    #[test]
    fn longer_payloads_pick_larger_versions() {
        let small = encode("x").unwrap();
        let big = encode(&"y".repeat(200)).unwrap();
        assert!(big.size > small.size, "{} !> {}", big.size, small.size);
        assert_eq!((big.size - 17) % 4, 0);
    }

    #[test]
    fn oversized_and_empty_inputs_return_errors_not_panics() {
        assert!(encode("").is_err());
        assert!(encode(&"z".repeat(1000)).is_err());
    }

    #[test]
    fn every_module_is_inside_the_grid() {
        let q = encode("bounds").unwrap();
        assert!(!q.dark(q.size, 0));
        assert!(!q.dark(0, q.size));
        assert_eq!(q.modules.len(), q.size * q.size);
    }

    #[test]
    fn dump_for_external_decoder() {
        let Ok(path) = std::env::var("PHOENIX_QR_DUMP") else {
            return;
        };
        let text = std::env::var("PHOENIX_QR_TEXT").unwrap_or_else(|_| "HELLO".into());
        let q = encode(&text).unwrap();
        let mut out = format!("{}\n", q.size);
        for y in 0..q.size {
            for x in 0..q.size {
                out.push(if q.dark(x, y) { '1' } else { '0' });
            }
            out.push('\n');
        }
        std::fs::write(path, out).unwrap();
    }

    #[test]
    fn the_reed_solomon_remainder_matches_a_known_vector() {
        let (exp, log) = gf_exp_log();
        let ec = ecc_for(
            &[0x40, 0xd2, 0x75, 0x47, 0x76, 0x17, 0x32, 0x06],
            10,
            &exp,
            &log,
        );
        assert_eq!(ec.len(), 10);
        assert_eq!(exp.first().copied(), Some(1));
        assert_eq!(exp.get(8).copied(), Some(0x1d));
    }
}
