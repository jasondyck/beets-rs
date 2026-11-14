//! CPython's `random.Random`, as langdetect uses it.
//!
//! langdetect 1.0.9 seeds a `random.Random()` with 0 and then draws
//! `gauss()` and `choice()` values; its result is deterministic only if the
//! same Mersenne Twister stream is reproduced. This is CPython's
//! `_random` module: `init_genrand`/`init_by_array` for an int seed,
//! `genrand_uint32` (tempered MT19937), `random()` (53 bits from two draws),
//! `getrandbits()`/`_randbelow()` (what `choice()` uses), and the cached
//! Box-Muller pair behind `gauss()` (Python 3.13's `random.py`).

const N: usize = 624;
const M: usize = 397;
const MATRIX_A: u32 = 0x9908_b0df;
const UPPER_MASK: u32 = 0x8000_0000;
const LOWER_MASK: u32 = 0x7fff_ffff;

pub struct PyRandom {
    mt: [u32; N],
    index: usize,
    gauss_next: Option<f64>,
}

impl PyRandom {
    /// `random.Random()`: unseeded (only [`seed`](Self::seed) is used here).
    pub fn new() -> Self {
        PyRandom {
            mt: [0; N],
            index: N,
            gauss_next: None,
        }
    }

    /// `Random.seed(int)`: `init_by_array` over the seed's 32-bit chunks
    /// (little-endian, of the absolute value; at least one chunk, so 0 is
    /// `[0]`), which is what CPython does for an int seed.
    pub fn seed(&mut self, seed: i64) {
        let mut key = vec![];
        let mut n = seed.unsigned_abs();
        loop {
            key.push((n & 0xffff_ffff) as u32);
            n >>= 32;
            if n == 0 {
                break;
            }
        }
        self.init_by_array(&key);
        self.gauss_next = None; // CPython's seed() resets it
    }

    fn init_genrand(&mut self, s: u32) {
        self.mt[0] = s;
        for i in 1..N {
            self.mt[i] =
                1_812_433_253u32.wrapping_mul(self.mt[i - 1] ^ (self.mt[i - 1] >> 30)) + i as u32;
        }
        self.index = N;
    }

    fn init_by_array(&mut self, key: &[u32]) {
        self.init_genrand(19_650_218);
        let mut i = 1;
        let mut j = 0;
        let mut k = N.max(key.len());
        while k > 0 {
            self.mt[i] = (self.mt[i]
                ^ (self.mt[i - 1] ^ (self.mt[i - 1] >> 30)).wrapping_mul(1_664_525))
            .wrapping_add(key[j])
            .wrapping_add(j as u32);
            i += 1;
            j += 1;
            if i >= N {
                self.mt[0] = self.mt[N - 1];
                i = 1;
            }
            if j >= key.len() {
                j = 0;
            }
            k -= 1;
        }
        k = N - 1;
        while k > 0 {
            self.mt[i] = (self.mt[i]
                ^ (self.mt[i - 1] ^ (self.mt[i - 1] >> 30)).wrapping_mul(1_566_083_941))
            .wrapping_sub(i as u32);
            i += 1;
            if i >= N {
                self.mt[0] = self.mt[N - 1];
                i = 1;
            }
            k -= 1;
        }
        self.mt[0] = 0x8000_0000;
        // CPython's init_by_array leaves the generator ready to refill.
        self.index = N;
    }

    fn genrand_uint32(&mut self) -> u32 {
        if self.index >= N {
            for kk in 0..N {
                let y = (self.mt[kk] & UPPER_MASK) | (self.mt[(kk + 1) % N] & LOWER_MASK);
                self.mt[kk] =
                    self.mt[(kk + M) % N] ^ (y >> 1) ^ if y & 1 != 0 { MATRIX_A } else { 0 };
            }
            self.index = 0;
        }
        let mut y = self.mt[self.index];
        self.index += 1;
        y ^= y >> 11;
        y ^= (y << 7) & 0x9d2c_5680;
        y ^= (y << 15) & 0xefc6_0000;
        y ^= y >> 18;
        y
    }

    /// `random()`: 53 random bits from two 32-bit draws.
    pub fn random(&mut self) -> f64 {
        let a = f64::from(self.genrand_uint32() >> 5);
        let b = f64::from(self.genrand_uint32() >> 6);
        (a * 67_108_864.0 + b) * (1.0 / 9_007_199_254_740_992.0)
    }

    /// `getrandbits(k)` (its `k <= 32` fast path).
    fn getrandbits(&mut self, k: u32) -> u64 {
        u64::from(self.genrand_uint32() >> (32 - k))
    }

    /// `_randbelow_with_getrandbits(n)`, which `choice()` uses.
    fn randbelow(&mut self, n: u64) -> u64 {
        let k = 64 - n.leading_zeros(); // n.bit_length()
        loop {
            let r = self.getrandbits(k);
            if r < n {
                return r;
            }
        }
    }

    /// `random.choice(seq)` for a non-empty slice.
    pub fn choice<'a, T>(&mut self, seq: &'a [T]) -> &'a T {
        let i = self.randbelow(seq.len() as u64) as usize;
        &seq[i]
    }

    /// `random.gauss(0.0, 1.0)`: Box-Muller with Python's cached second
    /// value.
    pub fn gauss(&mut self) -> f64 {
        if let Some(z) = self.gauss_next.take() {
            return z;
        }
        let x2pi = self.random() * std::f64::consts::TAU;
        let g2rad = (-2.0 * (1.0 - self.random()).ln()).sqrt();
        let z = x2pi.cos() * g2rad;
        self.gauss_next = Some(x2pi.sin() * g2rad);
        z
    }
}

impl Default for PyRandom {
    fn default() -> Self {
        Self::new()
    }
}
