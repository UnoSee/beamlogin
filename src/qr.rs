//! QR code encoder (byte mode, versions 1-40), following ISO/IEC 18004.
//! A line-for-line port of the Python Beamlogin encoder, so both produce identical codes.

const ECC_CODEWORDS_PER_BLOCK: [[i32; 41]; 4] = [
    // L
    [-1, 7, 10, 15, 20, 26, 18, 20, 24, 30, 18, 20, 24, 26, 30, 22, 24, 28, 30, 28, 28, 28, 28, 30, 30, 26, 28, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30],
    // M
    [-1, 10, 16, 26, 18, 24, 16, 18, 22, 22, 26, 30, 22, 22, 24, 24, 28, 28, 26, 26, 26, 26, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28],
    // Q
    [-1, 13, 22, 18, 26, 18, 24, 18, 22, 20, 24, 28, 26, 24, 20, 30, 24, 28, 28, 26, 30, 28, 30, 30, 30, 30, 28, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30],
    // H
    [-1, 17, 28, 22, 16, 22, 28, 26, 26, 24, 28, 24, 28, 22, 24, 24, 30, 28, 28, 26, 28, 30, 24, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30],
];
const NUM_ERROR_CORRECTION_BLOCKS: [[i32; 41]; 4] = [
    [-1, 1, 1, 1, 1, 1, 2, 2, 2, 2, 4, 4, 4, 4, 4, 6, 6, 6, 6, 7, 8, 8, 9, 9, 10, 12, 12, 12, 13, 14, 15, 16, 17, 18, 19, 19, 20, 21, 22, 24, 25],
    [-1, 1, 1, 1, 2, 2, 4, 4, 4, 5, 5, 5, 8, 9, 9, 10, 10, 11, 13, 14, 16, 17, 17, 18, 20, 21, 23, 25, 26, 28, 29, 31, 33, 35, 37, 38, 40, 43, 45, 47, 49],
    [-1, 1, 1, 2, 2, 4, 4, 6, 6, 8, 8, 8, 10, 12, 16, 12, 17, 16, 18, 21, 20, 23, 23, 25, 27, 29, 34, 34, 35, 38, 40, 43, 45, 48, 51, 53, 56, 59, 62, 65, 68],
    [-1, 1, 1, 2, 4, 4, 4, 5, 6, 8, 8, 11, 11, 16, 16, 18, 16, 19, 21, 25, 25, 25, 34, 30, 32, 35, 37, 40, 42, 45, 48, 51, 54, 57, 60, 63, 66, 70, 74, 77, 81],
];

fn ecl_index(ecl: char) -> usize {
    match ecl {
        'L' => 0,
        'Q' => 2,
        'H' => 3,
        _ => 1,
    }
}

fn ecl_format_bits(ecl: char) -> u32 {
    match ecl {
        'L' => 1,
        'Q' => 3,
        'H' => 2,
        _ => 0,
    }
}

fn num_raw_data_modules(ver: usize) -> usize {
    let v = ver as i64;
    let mut result = (16 * v + 128) * v + 64;
    if ver >= 2 {
        let numalign = v / 7 + 2;
        result -= (25 * numalign - 10) * numalign - 55;
        if ver >= 7 {
            result -= 36;
        }
    }
    result as usize
}

fn num_data_codewords(ver: usize, ecl: char) -> usize {
    let e = ecl_index(ecl);
    num_raw_data_modules(ver) / 8
        - (ECC_CODEWORDS_PER_BLOCK[e][ver] * NUM_ERROR_CORRECTION_BLOCKS[e][ver]) as usize
}

fn gf_mul(x: u8, y: u8) -> u8 {
    let mut z: u32 = 0;
    for i in (0..8).rev() {
        z = (z << 1) ^ ((z >> 7) * 0x11D);
        z ^= ((y as u32 >> i) & 1) * x as u32;
    }
    z as u8
}

fn rs_divisor(degree: usize) -> Vec<u8> {
    let mut result = vec![0u8; degree];
    result[degree - 1] = 1;
    let mut root: u8 = 1;
    for _ in 0..degree {
        for j in 0..degree {
            result[j] = gf_mul(result[j], root);
            if j + 1 < degree {
                result[j] ^= result[j + 1];
            }
        }
        root = gf_mul(root, 0x02);
    }
    result
}

fn rs_remainder(data: &[u8], divisor: &[u8]) -> Vec<u8> {
    let mut result = vec![0u8; divisor.len()];
    for &b in data {
        let factor = b ^ result.remove(0);
        result.push(0);
        for (i, &coef) in divisor.iter().enumerate() {
            result[i] ^= gf_mul(coef, factor);
        }
    }
    result
}

fn bit(x: u32, i: u32) -> bool {
    (x >> i) & 1 != 0
}

fn mask_fn(m: u8, x: usize, y: usize) -> bool {
    match m {
        0 => (x + y) % 2 == 0,
        1 => y % 2 == 0,
        2 => x % 3 == 0,
        3 => (x + y) % 3 == 0,
        4 => (x / 3 + y / 2) % 2 == 0,
        5 => x * y % 2 + x * y % 3 == 0,
        6 => (x * y % 2 + x * y % 3) % 2 == 0,
        _ => ((x + y) % 2 + x * y % 3) % 2 == 0,
    }
}

pub type Matrix = Vec<Vec<bool>>;

struct Grid {
    size: usize,
    mods: Matrix,
    isfunc: Matrix,
}

impl Grid {
    fn setf(&mut self, x: usize, y: usize, dark: bool) {
        self.mods[y][x] = dark;
        self.isfunc[y][x] = true;
    }

    fn draw_format(&mut self, ecl: char, m: u8) {
        let size = self.size;
        let d = ecl_format_bits(ecl) << 3 | m as u32;
        let mut rem = d;
        for _ in 0..10 {
            rem = (rem << 1) ^ ((rem >> 9) * 0x537);
        }
        let fb = (d << 10 | rem) ^ 0x5412;
        for i in 0..6 {
            self.setf(8, i, bit(fb, i as u32));
        }
        self.setf(8, 7, bit(fb, 6));
        self.setf(8, 8, bit(fb, 7));
        self.setf(7, 8, bit(fb, 8));
        for i in 9..15 {
            self.setf(14 - i, 8, bit(fb, i as u32));
        }
        for i in 0..8 {
            self.setf(size - 1 - i, 8, bit(fb, i as u32));
        }
        for i in 8..15 {
            self.setf(8, size - 15 + i, bit(fb, i as u32));
        }
        self.setf(8, size - 8, true);
    }

    fn apply_mask(&mut self, m: u8) {
        for y in 0..self.size {
            for x in 0..self.size {
                if !self.isfunc[y][x] && mask_fn(m, x, y) {
                    self.mods[y][x] = !self.mods[y][x];
                }
            }
        }
    }
}

/// Encode bytes as a QR matrix (rows of modules, true = dark).
pub fn encode(data: &[u8], ecl: char, version: Option<usize>, mask: Option<u8>) -> Result<Matrix, String> {
    let mut chosen = None;
    for ver in 1..=40usize {
        if let Some(v) = version {
            if ver != v {
                continue;
            }
        }
        let ccbits = if ver <= 9 { 8 } else { 16 };
        let capacity = num_data_codewords(ver, ecl) * 8;
        if data.len() < (1usize << ccbits) && 4 + ccbits + data.len() * 8 <= capacity {
            chosen = Some((ver, ccbits, capacity));
            break;
        }
    }
    let (ver, ccbits, capacity) = chosen.ok_or_else(|| "data too long for a QR code".to_string())?;

    let mut bits: Vec<u8> = Vec::new();
    let append = |bits: &mut Vec<u8>, val: u32, n: usize| {
        for i in (0..n).rev() {
            bits.push(((val >> i) & 1) as u8);
        }
    };
    append(&mut bits, 0b0100, 4);
    append(&mut bits, data.len() as u32, ccbits);
    for &b in data {
        append(&mut bits, b as u32, 8);
    }
    let term = 4.min(capacity - bits.len());
    append(&mut bits, 0, term);
    let pad_bits = (8 - bits.len() % 8) % 8;
    append(&mut bits, 0, pad_bits);
    let mut pad: u32 = 0xEC;
    while bits.len() < capacity {
        append(&mut bits, pad, 8);
        pad ^= 0xEC ^ 0x11;
    }
    let codewords: Vec<u8> = bits.chunks(8).map(|c| c.iter().fold(0u8, |acc, &b| acc << 1 | b)).collect();

    // Error correction and interleaving.
    let e = ecl_index(ecl);
    let numblocks = NUM_ERROR_CORRECTION_BLOCKS[e][ver] as usize;
    let blockecclen = ECC_CODEWORDS_PER_BLOCK[e][ver] as usize;
    let rawcodewords = num_raw_data_modules(ver) / 8;
    let numshortblocks = numblocks - rawcodewords % numblocks;
    let shortblocklen = rawcodewords / numblocks;
    let divisor = rs_divisor(blockecclen);
    let mut blocks: Vec<Vec<u8>> = Vec::new();
    let mut k = 0;
    for i in 0..numblocks {
        let len = shortblocklen - blockecclen + if i < numshortblocks { 0 } else { 1 };
        let mut dat: Vec<u8> = codewords[k..k + len].to_vec();
        k += len;
        let ecc = rs_remainder(&dat, &divisor);
        if i < numshortblocks {
            dat.push(0);
        }
        dat.extend(ecc);
        blocks.push(dat);
    }
    let mut allcw: Vec<u8> = Vec::new();
    for i in 0..blocks[0].len() {
        for (j, blk) in blocks.iter().enumerate() {
            if i != shortblocklen - blockecclen || j >= numshortblocks {
                allcw.push(blk[i]);
            }
        }
    }

    let size = ver * 4 + 17;
    let mut g = Grid { size, mods: vec![vec![false; size]; size], isfunc: vec![vec![false; size]; size] };

    for i in 0..size {
        g.setf(6, i, i % 2 == 0);
        g.setf(i, 6, i % 2 == 0);
    }
    for &(cx, cy) in &[(3i64, 3i64), (size as i64 - 4, 3), (3, size as i64 - 4)] {
        for dy in -4i64..=4 {
            for dx in -4i64..=4 {
                let (xx, yy) = (cx + dx, cy + dy);
                if xx >= 0 && xx < size as i64 && yy >= 0 && yy < size as i64 {
                    let d = dx.abs().max(dy.abs());
                    g.setf(xx as usize, yy as usize, d != 2 && d != 4);
                }
            }
        }
    }
    if ver > 1 {
        let numalign = ver / 7 + 2;
        let step = (ver * 8 + numalign * 3 + 5) / (numalign * 4 - 4) * 2;
        let mut pos: Vec<usize> = vec![6];
        let mut rest: Vec<usize> = (0..numalign - 1).map(|i| size - 7 - i * step).collect();
        rest.reverse();
        pos.extend(rest);
        let last = pos.len() - 1;
        for (i, &ax) in pos.iter().enumerate() {
            for (j, &ay) in pos.iter().enumerate() {
                if (i == 0 && j == 0) || (i == 0 && j == last) || (i == last && j == 0) {
                    continue;
                }
                for dy in -2i64..=2 {
                    for dx in -2i64..=2 {
                        let d = dx.abs().max(dy.abs());
                        g.setf((ax as i64 + dx) as usize, (ay as i64 + dy) as usize, d != 1);
                    }
                }
            }
        }
    }

    g.draw_format(ecl, 0);
    if ver >= 7 {
        let mut rem = ver as u32;
        for _ in 0..12 {
            rem = (rem << 1) ^ ((rem >> 11) * 0x1F25);
        }
        let vb = (ver as u32) << 12 | rem;
        for i in 0..18u32 {
            let b = bit(vb, i);
            let a = size - 11 + (i % 3) as usize;
            let c = (i / 3) as usize;
            g.setf(a, c, b);
            g.setf(c, a, b);
        }
    }

    let mut i = 0usize;
    let mut right = size as i64 - 1;
    while right >= 1 {
        if right == 6 {
            right = 5;
        }
        for vert in 0..size {
            for j in 0..2 {
                let x = (right - j) as usize;
                let upward = ((right + 1) & 2) == 0;
                let y = if upward { size - 1 - vert } else { vert };
                if !g.isfunc[y][x] && i < allcw.len() * 8 {
                    g.mods[y][x] = bit(allcw[i >> 3] as u32, 7 - (i & 7) as u32);
                    i += 1;
                }
            }
        }
        right -= 2;
    }

    let mask = match mask {
        Some(m) => m,
        None => {
            let (mut best, mut bestpen) = (0u8, i64::MAX);
            for m in 0..8u8 {
                g.apply_mask(m);
                g.draw_format(ecl, m);
                let pen = penalty(&g.mods);
                if pen < bestpen {
                    best = m;
                    bestpen = pen;
                }
                g.apply_mask(m);
            }
            best
        }
    };
    g.apply_mask(mask);
    g.draw_format(ecl, mask);
    Ok(g.mods)
}

fn penalty(mods: &Matrix) -> i64 {
    let size = mods.len();
    let mut result: i64 = 0;
    let mut lines: Vec<Vec<bool>> = mods.clone();
    for x in 0..size {
        lines.push((0..size).map(|y| mods[y][x]).collect());
    }
    let pat = [true, false, true, true, true, false, true];
    for line in &lines {
        let mut run = 0i64;
        let mut prev: Option<bool> = None;
        for &c in line {
            if Some(c) == prev {
                run += 1;
            } else {
                if run >= 5 {
                    result += 3 + run - 5;
                }
                run = 1;
                prev = Some(c);
            }
        }
        if run >= 5 {
            result += 3 + run - 5;
        }
        let mut padded = vec![false; 4];
        padded.extend(line.iter().copied());
        padded.extend([false; 4]);
        for k in 0..padded.len() - 6 {
            if padded[k..k + 7] == pat {
                let before = padded[k.saturating_sub(4)..k].iter().any(|&b| b);
                let after_end = (k + 11).min(padded.len());
                let after = padded[(k + 7).min(padded.len())..after_end].iter().any(|&b| b);
                if !before || !after {
                    result += 40;
                }
            }
        }
    }
    for y in 0..size - 1 {
        for x in 0..size - 1 {
            let c = mods[y][x];
            if c == mods[y][x + 1] && c == mods[y + 1][x] && c == mods[y + 1][x + 1] {
                result += 3;
            }
        }
    }
    let total = (size * size) as i64;
    let dark: i64 = mods.iter().map(|r| r.iter().filter(|&&b| b).count() as i64).sum();
    let k = ((dark * 20 - total * 10).abs() + total - 1).div_euclid(total) - 1;
    result + k * 10
}

/// Render for a terminal: dark modules on a light background.
pub fn render(mods: &Matrix, quiet: usize, style: &str) -> String {
    let size = mods.len() as i64;
    let q = quiet as i64;
    let dark = |x: i64, y: i64| x >= 0 && x < size && y >= 0 && y < size && mods[y as usize][x as usize];
    let rng: Vec<i64> = (-q..size + q).collect();
    let reset = "\x1b[0m";
    let mut lines: Vec<String> = Vec::new();
    if style == "half" {
        let mut i = 0;
        while i < rng.len() {
            let y0 = rng[i];
            let y1 = rng.get(i + 1).copied();
            let mut row = String::new();
            for &x in &rng {
                let t = dark(x, y0);
                let b = y1.map(|y| dark(x, y)).unwrap_or(false);
                row.push(if t && b {
                    '\u{2588}'
                } else if t {
                    '\u{2580}'
                } else if b {
                    '\u{2584}'
                } else {
                    ' '
                });
            }
            lines.push(format!("\x1b[30;47m{}{}", row, reset));
            i += 2;
        }
    } else {
        for &y in &rng {
            let row: String = rng.iter().map(|&x| if dark(x, y) { "\x1b[40m  " } else { "\x1b[47m  " }).collect();
            lines.push(format!("{}{}", row, reset));
        }
    }
    lines.join("\n")
}
