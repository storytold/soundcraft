//! How an offline render splits its range into blocks. Plugins must handle any block size from 1
//! up to the size they were prepared for, and many bugs only show at odd or changing sizes, so a
//! render can use one size, cycle through a list, or draw sizes from a seeded generator (the
//! same seed gives the same sizes, so a failure can be reproduced).

/// The largest block the mix engine processes (`MixEngine::new` clamps to it).
pub const MAX_BLOCK: usize = 16_384;
/// Most sizes a [`BlockSizes::Cycle`] may list.
pub const MAX_CYCLE: usize = 4096;
/// Most blocks one render job should take ([`BlockSizes::blocks_for`]): each block costs time
/// whatever its size (a few to tens of µs with many tracks), so 1-sample blocks over hours of
/// audio would keep the engine busy for hours. Ten million is minutes at worst.
pub const MAX_RENDER_BLOCKS: u64 = 10_000_000;

/// The block sizes of a render, each 1..=[`MAX_BLOCK`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockSizes {
    /// Every block this size (the last one may be shorter).
    Fixed(usize),
    /// The sizes in turn, from the start again after the last.
    Cycle(Vec<usize>),
    /// Sizes drawn evenly from `min..=max` by a generator seeded with `seed`.
    Random { min: usize, max: usize, seed: u64 },
}

impl BlockSizes {
    /// One size, clamped to 1..=[`MAX_BLOCK`].
    pub fn fixed(n: usize) -> Self {
        BlockSizes::Fixed(n.clamp(1, MAX_BLOCK))
    }

    /// Why these sizes can't be used (`None` when they can).
    pub fn problem(&self) -> Option<String> {
        let in_range = |n: usize| (1..=MAX_BLOCK).contains(&n);
        match self {
            BlockSizes::Fixed(n) if !in_range(*n) => Some(format!("block sizes must be 1 to {MAX_BLOCK}")),
            BlockSizes::Cycle(v) if v.is_empty() || v.len() > MAX_CYCLE => Some(format!("a block size list holds 1 to {MAX_CYCLE} sizes")),
            BlockSizes::Cycle(v) if !v.iter().all(|n| in_range(*n)) => Some(format!("block sizes must be 1 to {MAX_BLOCK}")),
            BlockSizes::Random { min, max, .. } if !in_range(*min) || !in_range(*max) || min > max => {
                Some(format!("random block sizes need 1 <= min <= max <= {MAX_BLOCK}"))
            }
            _ => None,
        }
    }

    /// The largest block these sizes produce: what plugins must be prepared for.
    pub fn max(&self) -> usize {
        let n = match self {
            BlockSizes::Fixed(n) => *n,
            BlockSizes::Cycle(v) => v.iter().copied().max().unwrap_or(1),
            BlockSizes::Random { max, .. } => *max,
        };
        n.clamp(1, MAX_BLOCK)
    }

    /// The average block size.
    pub fn mean(&self) -> f64 {
        let n = match self {
            BlockSizes::Fixed(n) => *n as f64,
            BlockSizes::Cycle(v) if !v.is_empty() => v.iter().sum::<usize>() as f64 / v.len() as f64,
            BlockSizes::Cycle(_) => 1.0,
            BlockSizes::Random { min, max, .. } => (*min as f64 + *max as f64) / 2.0,
        };
        n.clamp(1.0, MAX_BLOCK as f64)
    }

    /// About how many blocks rendering `samples` takes (on average for random sizes).
    pub fn blocks_for(&self, samples: u64) -> u64 {
        (samples as f64 / self.mean()).ceil() as u64
    }

    /// The sizes in order, without end.
    pub fn iter(&self) -> BlockIter {
        let state = match self {
            BlockSizes::Random { seed, .. } => *seed,
            _ => 0,
        };
        BlockIter { sizes: self.clone(), at: 0, state }
    }
}

impl Default for BlockSizes {
    /// 1024-sample blocks, the size offline renders have always used.
    fn default() -> Self {
        BlockSizes::Fixed(1024)
    }
}

/// An endless sequence of block sizes (see [`BlockSizes::iter`]); every size is 1..=[`MAX_BLOCK`].
#[derive(Debug, Clone)]
pub struct BlockIter {
    sizes: BlockSizes,
    at: usize,
    state: u64,
}

impl BlockIter {
    /// The sizes this sequence follows.
    pub fn sizes(&self) -> &BlockSizes {
        &self.sizes
    }

    pub fn next_size(&mut self) -> usize {
        let n = match &self.sizes {
            BlockSizes::Fixed(n) => *n,
            BlockSizes::Cycle(v) => {
                let n = v.get(self.at % v.len().max(1)).copied().unwrap_or(1);
                self.at = self.at.wrapping_add(1);
                n
            }
            BlockSizes::Random { min, max, .. } => {
                let span = max.saturating_sub(*min).saturating_add(1) as u64;
                min.saturating_add((self.splitmix() % span.max(1)) as usize)
            }
        };
        n.clamp(1, MAX_BLOCK)
    }

    /// SplitMix64: small, fast and the same on every platform.
    fn splitmix(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn take(b: &BlockSizes, n: usize) -> Vec<usize> {
        let mut it = b.iter();
        (0..n).map(|_| it.next_size()).collect()
    }

    #[test]
    fn fixed_and_cycled_sizes() {
        assert_eq!(take(&BlockSizes::fixed(37), 3), [37, 37, 37]);
        assert_eq!(BlockSizes::fixed(0), BlockSizes::Fixed(1));
        assert_eq!(BlockSizes::fixed(1 << 20).max(), MAX_BLOCK);
        let c = BlockSizes::Cycle(vec![1, 512, 37]);
        assert_eq!(take(&c, 7), [1, 512, 37, 1, 512, 37, 1]);
        assert_eq!(c.max(), 512);
        assert_eq!(BlockSizes::default(), BlockSizes::Fixed(1024));
        assert_eq!(c.blocks_for(55_000), 300, "mean 183.3");
        assert_eq!(BlockSizes::fixed(1).blocks_for(48_000), 48_000);
        assert_eq!(BlockSizes::Random { min: 1, max: 99, seed: 0 }.blocks_for(500), 10);
    }

    #[test]
    fn random_sizes_stay_in_range_and_repeat_per_seed() {
        let r = BlockSizes::Random { min: 1, max: 64, seed: 7 };
        let a = take(&r, 2000);
        assert!(a.iter().all(|n| (1..=64).contains(n)));
        assert!(a.contains(&1) && a.contains(&64), "both ends are reached");
        assert_eq!(a, take(&r, 2000), "the same seed gives the same sizes");
        assert_ne!(a, take(&BlockSizes::Random { min: 1, max: 64, seed: 8 }, 2000));
        assert_eq!(take(&BlockSizes::Random { min: 5, max: 5, seed: 0 }, 3), [5, 5, 5]);
        assert_eq!(r.max(), 64);
    }

    #[test]
    fn unusable_sizes_are_reported() {
        assert!(BlockSizes::Fixed(0).problem().is_some());
        assert!(BlockSizes::Fixed(MAX_BLOCK + 1).problem().is_some());
        assert!(BlockSizes::Cycle(Vec::new()).problem().is_some());
        assert!(BlockSizes::Cycle(vec![1, 0]).problem().is_some());
        assert!(BlockSizes::Cycle(vec![1; MAX_CYCLE + 1]).problem().is_some());
        assert!(BlockSizes::Random { min: 9, max: 8, seed: 0 }.problem().is_some());
        assert!(BlockSizes::Random { min: 0, max: 8, seed: 0 }.problem().is_some());
        assert_eq!(BlockSizes::Random { min: 1, max: MAX_BLOCK, seed: u64::MAX }.problem(), None);
        assert_eq!(BlockSizes::Cycle(vec![1, MAX_BLOCK]).problem(), None);
    }
}
