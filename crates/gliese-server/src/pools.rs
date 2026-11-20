//! Buffer pools: a few large shared objects per program that window buffers are cut out of (see `gliese_proto`, "Buffers"). On
//! Metallium-OS every object costs a capability slot (32 a task) and a mapping (64), in the compositor and in the program; with pools
//! that is one per pool, not one per window.
//!
//! First fit in page units; freed ranges merge with their neighbours; a pool with nothing left in it is destroyed.
use crate::{Host, ShmId, State, Task};
use alloc::vec;
use alloc::vec::Vec;
use gliese_proto::{MAX_POOLS, MAX_POOL_PAGES};

/// The smallest pool made (most windows fit in a few of these).
const MIN_POOL_PAGES: usize = 1024;

pub(crate) struct Pool {
    /// Never reused while the compositor runs.
    pub id: u32,
    pub shm: ShmId,
    pub pages: u32,
    /// Free ranges (start, length) in pages, sorted, never touching.
    pub free: Vec<(u32, u32)>,
    /// The slot each task of the program has it in.
    pub grants: Vec<(Task, u32)>,
}

impl Pool {
    fn alloc(&mut self, pages: u32) -> Option<u32> {
        let i = self.free.iter().position(|(_, len)| *len >= pages)?;
        let (start, len) = self.free[i];
        if len == pages {
            self.free.remove(i);
        } else {
            self.free[i] = (start + pages, len - pages);
        }
        Some(start)
    }

    fn release(&mut self, start: u32, pages: u32) {
        let i = self.free.iter().position(|(s, _)| *s > start).unwrap_or(self.free.len());
        self.free.insert(i, (start, pages));
        // merge with the next, then with the previous
        if i + 1 < self.free.len() && self.free[i].0 + self.free[i].1 == self.free[i + 1].0 {
            self.free[i].1 += self.free[i + 1].1;
            self.free.remove(i + 1);
        }
        if i > 0 && self.free[i - 1].0 + self.free[i - 1].1 == self.free[i].0 {
            self.free[i - 1].1 += self.free[i].1;
            self.free.remove(i);
        }
    }

    fn is_empty(&self) -> bool {
        self.free.len() == 1 && self.free[0] == (0, self.pages)
    }
}

impl State {
    /// Room for `pages` in a pool of session `sess`: (pool id, offset in pages). A new pool is made when none has room.
    pub(crate) fn alloc_buf(&mut self, host: &mut dyn Host, sess: usize, pages: usize) -> Result<(u32, u32), i64> {
        if pages > MAX_POOL_PAGES {
            return Err(gliese_proto::E_NOMEM);
        }
        let s = self.sessions[sess].as_mut().ok_or(gliese_proto::E_BADWIN)?;
        for p in s.pools.iter_mut() {
            if let Some(off) = p.alloc(pages as u32) {
                return Ok((p.id, off));
            }
        }
        if s.pools.len() >= MAX_POOLS {
            return Err(gliese_proto::E_NOMEM);
        }
        // a new pool: room for this buffer and a few more like it
        let size = (pages * 2).clamp(MIN_POOL_PAGES, MAX_POOL_PAGES).max(pages);
        if self.pages_used + size > self.limits.buffer_pages {
            // (the budget: a pool just big enough, if that fits)
            if self.pages_used + pages > self.limits.buffer_pages {
                return Err(gliese_proto::E_NOMEM);
            }
            return self.new_pool(host, sess, pages, pages);
        }
        self.new_pool(host, sess, size, pages)
    }

    fn new_pool(&mut self, host: &mut dyn Host, sess: usize, size: usize, pages: usize) -> Result<(u32, u32), i64> {
        let shm = host.shm_create(size)?;
        self.pages_used += size;
        self.next_pool += 1;
        let id = self.next_pool;
        let mut p = Pool { id, shm, pages: size as u32, free: vec![(0, size as u32)], grants: Vec::new() };
        let off = p.alloc(pages as u32).expect("a new pool has room");
        self.sessions[sess].as_mut().expect("there").pools.push(p);
        Ok((id, off))
    }

    /// Give `pages` at `offset` of pool `id` back; a pool left empty is destroyed.
    pub(crate) fn free_buf(&mut self, host: &mut dyn Host, sess: usize, id: u32, offset: u32, pages: usize) {
        let Some(s) = self.sessions.get_mut(sess).and_then(|s| s.as_mut()) else { return };
        let Some(i) = s.pools.iter().position(|p| p.id == id) else { return };
        s.pools[i].release(offset, pages as u32);
        if s.pools[i].is_empty() {
            let p = s.pools.remove(i);
            self.pages_used -= p.pages as usize;
            host.shm_destroy(p.shm);
        }
    }

    /// The slot `task` has pool `id` in (granted now if it has none yet).
    pub(crate) fn pool_slot(&mut self, host: &mut dyn Host, sess: usize, id: u32, task: Task) -> Result<u32, i64> {
        let s = self.sessions[sess].as_mut().ok_or(gliese_proto::E_BADWIN)?;
        let p = s.pools.iter_mut().find(|p| p.id == id).ok_or(gliese_proto::E_BADWIN)?;
        if let Some((_, slot)) = p.grants.iter().find(|(t, _)| *t == task) {
            return Ok(*slot);
        }
        let slot = host.shm_grant(p.shm, task)?;
        p.grants.push((task, slot));
        Ok(slot)
    }

    /// The object pool `id` of session `sess` is.
    pub(crate) fn pool_shm(&self, sess: usize, id: u32) -> Option<ShmId> {
        self.sessions.get(sess)?.as_ref()?.pools.iter().find(|p| p.id == id).map(|p| p.shm)
    }

    /// Destroy every pool of a session (it ended).
    pub(crate) fn drop_pools(&mut self, host: &mut dyn Host, sess: usize) {
        let Some(s) = self.sessions.get_mut(sess).and_then(|s| s.as_mut()) else { return };
        for p in core::mem::take(&mut s.pools) {
            self.pages_used -= p.pages as usize;
            host.shm_destroy(p.shm);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_fit_and_merging() {
        let mut p = Pool { id: 1, shm: 0, pages: 100, free: vec![(0, 100)], grants: Vec::new() };
        let a = p.alloc(30).unwrap();
        let b = p.alloc(30).unwrap();
        let c = p.alloc(30).unwrap();
        assert_eq!((a, b, c), (0, 30, 60));
        assert_eq!(p.alloc(20), None);
        p.release(b, 30);
        assert_eq!(p.alloc(20), Some(30), "the hole is reused");
        p.release(30, 20);
        p.release(a, 30);
        assert_eq!(p.free, vec![(0, 60), (90, 10)], "neighbours merge");
        p.release(c, 30);
        assert!(p.is_empty());
    }
}
