//! Siblings opened to idle threads (young brothers wait): the bookkeeping both searches share.

use std::sync::atomic::Ordering::{AcqRel, Acquire, Relaxed, Release};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, AtomicUsize};
use std::sync::{Arc, RwLock};

/// A stop that also trips when any ancestor's does.
pub(crate) struct AbortChain {
    flag: AtomicBool,
    parent: Option<Arc<AbortChain>>,
}

impl AbortChain {
    pub(crate) fn child(parent: Option<Arc<AbortChain>>) -> Arc<AbortChain> {
        Arc::new(AbortChain {
            flag: AtomicBool::new(false),
            parent,
        })
    }

    pub(crate) fn raise(&self) {
        self.flag.store(true, Relaxed);
    }

    pub(crate) fn stopped(&self) -> bool {
        let mut n = self;
        loop {
            if n.flag.load(Relaxed) {
                return true;
            }
            match &n.parent {
                Some(p) => n = p,
                None => return false,
            }
        }
    }

    pub(crate) fn descends_from(&self, ancestor: &AbortChain) -> bool {
        let mut n = self;
        loop {
            if std::ptr::eq(n, ancestor) {
                return true;
            }
            match &n.parent {
                Some(p) => n = p,
                None => return false,
            }
        }
    }
}

const OPEN: u8 = 0;
const WRITING: u8 = 1;
const WRITTEN: u8 = 2;

struct Kid {
    taken: AtomicBool,
    /// The first search to finish the kid owns its value.
    state: AtomicU8,
    bits: AtomicU32,
    /// Searches of this kid now running; more than one when the owner takes over a slow helper's.
    running: AtomicU32,
    taken_over: AtomicBool,
    /// Stops every search of this kid once one of them has its value.
    stop: Arc<AbortChain>,
}

/// The kids of one fan, each searched once by whoever takes it first.
pub(crate) struct Kids {
    kids: Vec<Kid>,
    left: AtomicUsize,
    helpers: AtomicUsize,
    /// Nodes the fan's searches visited.
    pub(crate) nodes: AtomicU64,
    /// Raised on a cut, or with the node's own stop.
    pub(crate) stop: Arc<AbortChain>,
    /// Depth left at the kids: the largest is the nearest the root.
    depth: u32,
}

impl Kids {
    pub(crate) fn new(n: usize, depth: u32, stop: Arc<AbortChain>) -> Kids {
        Kids {
            kids: (0..n)
                .map(|_| Kid {
                    taken: AtomicBool::new(false),
                    state: AtomicU8::new(OPEN),
                    bits: AtomicU32::new(0),
                    running: AtomicU32::new(0),
                    taken_over: AtomicBool::new(false),
                    stop: AbortChain::child(Some(stop.clone())),
                })
                .collect(),
            left: AtomicUsize::new(n),
            helpers: AtomicUsize::new(0),
            nodes: AtomicU64::new(0),
            stop,
            depth,
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.kids.len()
    }

    /// Claims kid `k`; false if someone already has.
    pub(crate) fn take(&self, k: usize) -> bool {
        if self.kids[k].taken.swap(true, AcqRel) {
            return false;
        }
        self.left.fetch_sub(1, Relaxed);
        true
    }

    fn open(&self) -> bool {
        self.left.load(Relaxed) > 0 && !self.stop.stopped()
    }

    /// Whether every kid a search started on has its value.
    fn settled(&self) -> bool {
        self.kids
            .iter()
            .all(|k| k.running.load(Acquire) == 0 || k.state.load(Acquire) == WRITTEN)
    }

    /// Kid `k`'s value, once a search wrote it.
    pub(crate) fn result(&self, k: usize) -> Option<u32> {
        let kid = &self.kids[k];
        (kid.state.load(Acquire) == WRITTEN).then(|| kid.bits.load(Relaxed))
    }

    /// Searches kid `k` given its stop; the first finisher writes the value, stops the rest, and the fan too if `cuts`.
    pub(crate) fn run(
        &self,
        k: usize,
        search: impl FnOnce(&Arc<AbortChain>) -> u32,
        cuts: impl FnOnce(u32) -> bool,
    ) {
        let kid = &self.kids[k];
        kid.running.fetch_add(1, AcqRel);
        let bits = search(&kid.stop);
        if kid
            .state
            .compare_exchange(OPEN, WRITING, AcqRel, Acquire)
            .is_ok()
        {
            if cuts(bits) {
                self.stop.raise();
            }
            kid.bits.store(bits, Relaxed);
            kid.state.store(WRITTEN, Release);
            kid.stop.raise();
        }
        kid.running.fetch_sub(1, AcqRel);
    }

    pub(crate) fn depth(&self) -> u32 {
        self.depth
    }

    /// Searches every kid nobody has taken, in order, until the fan stops; true if it took one.
    pub(crate) fn search_free(&self, mut search: impl FnMut(usize)) -> bool {
        let mut ran = false;
        for k in 0..self.kids.len() {
            if self.stop.stopped() {
                break;
            }
            if self.take(k) {
                ran = true;
                search(k);
            }
        }
        ran
    }

    /// A kid a helper is still searching and nobody took over yet, now taken over by the caller.
    pub(crate) fn take_over(&self) -> Option<usize> {
        self.kids.iter().position(|kid| {
            kid.running.load(Acquire) > 0
                && kid.state.load(Acquire) == OPEN
                && !kid.taken_over.swap(true, AcqRel)
        })
    }
}

/// A search's fan: its kids plus whatever the search needs to search one.
pub(crate) trait HasKids: Send + Sync {
    fn kids(&self) -> &Kids;
}

/// The fans open right now, for idle threads to pick from.
pub(crate) struct FanList<F> {
    fans: RwLock<Vec<Arc<F>>>,
    n: AtomicUsize,
}

impl<F: HasKids> FanList<F> {
    pub(crate) fn new() -> FanList<F> {
        FanList {
            fans: RwLock::new(Vec::new()),
            n: AtomicUsize::new(0),
        }
    }

    pub(crate) fn add(&self, f: &Arc<F>) {
        self.fans.write().unwrap().push(f.clone());
        self.n.fetch_add(1, Relaxed);
    }

    pub(crate) fn any_open(&self) -> bool {
        self.n.load(Relaxed) > 0 && self.fans.read().unwrap().iter().any(|f| f.kids().open())
    }

    /// Joins the open fan nearest the root, under `under` if given: its siblings are the largest pieces of work.
    pub(crate) fn help(
        &self,
        under: Option<&AbortChain>,
        run: impl FnOnce(&Arc<F>) -> bool,
    ) -> bool {
        if self.n.load(Relaxed) == 0 {
            return false;
        }
        let pick = {
            let fans = self.fans.read().unwrap();
            fans.iter()
                .filter(|f| {
                    let k = f.kids();
                    k.open() && under.is_none_or(|a| k.stop.descends_from(a))
                })
                .max_by_key(|f| f.kids().depth)
                .map(|f| {
                    f.kids().helpers.fetch_add(1, AcqRel);
                    f.clone()
                })
        };
        let Some(f) = pick else {
            return false;
        };
        let ran = run(&f);
        f.kids().helpers.fetch_sub(1, AcqRel);
        ran
    }

    /// Takes back what nobody started and waits out the helpers, helping the fans under this one meanwhile.
    pub(crate) fn close(
        &self,
        f: &Arc<F>,
        mut help_under: impl FnMut(&AbortChain) -> bool,
        mut take_over: impl FnMut() -> bool,
    ) {
        let kids = f.kids();
        for k in 0..kids.len() {
            kids.take(k);
        }
        self.fans.write().unwrap().retain(|x| !Arc::ptr_eq(x, f));
        self.n.fetch_sub(1, Relaxed);
        let mut idle = 0u32;
        while kids.helpers.load(Acquire) > 0 && !kids.settled() {
            // Only fans below: one elsewhere would hold the owner past its result. A set-aside helper's kid is searched too.
            if help_under(&kids.stop) || take_over() {
                idle = 0;
                continue;
            }
            idle += 1;
            if idle < 64 {
                std::hint::spin_loop();
            } else {
                std::thread::yield_now();
            }
        }
    }
}
