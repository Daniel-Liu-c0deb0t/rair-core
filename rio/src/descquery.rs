/*
 * descquery.rs: Data structure that enables Query on open files by handles, or paddr.
 * Copyright (C) 2019  Oddcoder
 * This program is free software: you can redistribute it and/or modify
 * it under the terms of the GNU Lesser General Public License as published by
 * the Free Software Foundation, either version 3 of the License, or
 * (at your option) any later version.
 *
 * This program is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * GNU Lesser General Public License for more details.
 *
 * You should have received a copy of the GNU Lesser General Public License
 * along with this program.  If not, see <http://www.gnu.org/licenses/>.
 */
use desc::RIODesc;
use plugin::RIOPlugin;
use rtrees::ist::IST;
use std::cmp::{min, Reverse};
use std::collections::BinaryHeap;
use std::mem;
use utils::{IoError, IoMode};

pub struct RIODescQuery {
    hndl_to_descs: Vec<Option<RIODesc>>,  // key = hndl, value = RIODesc Should it exist
    paddr_to_hndls: IST<u64, u64>,        // key = closed range, value = hndl
    next_hndl: u64,                       // nxt handle to be used
    free_hndls: BinaryHeap<Reverse<u64>>, // list of free handles
}

impl RIODescQuery {
    pub fn new() -> RIODescQuery {
        RIODescQuery {
            hndl_to_descs: Vec::new(),
            paddr_to_hndls: IST::new(),
            next_hndl: 0,
            free_hndls: BinaryHeap::new(),
        }
    }
    // under the assumption that we will always have a free handle! I mean who can open 2^64 files!
    fn get_new_hndl(&mut self) -> u64 {
        let result;
        if let Some(Reverse(hndl)) = self.free_hndls.pop() {
            result = hndl;
        } else {
            result = self.next_hndl;
            self.next_hndl += 1;
        }
        return result;
    }
    fn register_handle(&mut self, plugin: &mut Box<dyn RIOPlugin>, uri: &str, flags: IoMode) -> Result<u64, IoError> {
        let mut desc = RIODesc::open(plugin, uri, flags)?;
        let hndl = self.get_new_hndl();
        desc.hndl = hndl;
        if hndl < self.hndl_to_descs.len() as u64 {
            self.hndl_to_descs[hndl as usize] = Some(desc);
        } else {
            self.hndl_to_descs.push(Some(desc));
        }
        return Ok(hndl);
    }
    fn deregister_hndl(&mut self, hndl: u64) -> Result<RIODesc, IoError> {
        if hndl >= self.hndl_to_descs.len() as u64 || self.hndl_to_descs[hndl as usize].is_none() {
            return Err(IoError::HndlNotFoundError);
        }
        let ret = mem::replace(&mut self.hndl_to_descs[hndl as usize], None).unwrap();
        self.free_hndls.push(Reverse(hndl));
        return Ok(ret);
    }
    pub fn close(&mut self, hndl: u64) -> Result<RIODesc, IoError> {
        let desc = self.deregister_hndl(hndl)?;
        self.paddr_to_hndls.delete_envelop(desc.paddr, desc.paddr + desc.size - 1);
        return Ok(desc);
    }
    pub fn register_open(&mut self, plugin: &mut Box<dyn RIOPlugin>, uri: &str, flags: IoMode) -> Result<u64, IoError> {
        let hndl = self.register_handle(plugin, uri, flags)?;
        let mut lo = 0;
        let size = self.hndl_to_descs[hndl as usize].as_ref().unwrap().size;
        loop {
            let overlaps = self.paddr_to_hndls.overlap(lo, lo + size - 1);
            if overlaps.is_empty() {
                break;
            }
            let last_hndl = *overlaps[overlaps.len() - 1];
            let last = self.hndl_to_descs[last_hndl as usize].as_ref().unwrap();
            lo = last.paddr + last.size;
        }
        self.hndl_to_descs[hndl as usize].as_mut().unwrap().paddr = lo;
        self.paddr_to_hndls.insert(lo, lo + size - 1, hndl);
        return Ok(hndl);
    }
    pub fn register_open_at(&mut self, plugin: &mut Box<dyn RIOPlugin>, uri: &str, flags: IoMode, at: u64) -> Result<u64, IoError> {
        let hndl = self.register_handle(plugin, uri, flags)?;
        let lo = at;
        let hi = at + self.hndl_to_descs[hndl as usize].as_ref().unwrap().size - 1;
        if !self.paddr_to_hndls.overlap(lo, hi).is_empty() {
            self.deregister_hndl(hndl).unwrap();
            return Err(IoError::AddressesOverlapError);
        }
        self.hndl_to_descs[hndl as usize].as_mut().unwrap().paddr = lo;
        self.paddr_to_hndls.insert(lo, hi, hndl);
        return Ok(hndl);
    }
    pub fn hndl_to_desc(&self, hndl: u64) -> Option<&RIODesc> {
        if hndl >= self.hndl_to_descs.len() as u64 {
            return None;
        }
        return self.hndl_to_descs[hndl as usize].as_ref();
    }
    pub fn hndl_to_mut_desc(&mut self, hndl: u64) -> Option<&mut RIODesc> {
        if hndl >= self.hndl_to_descs.len() as u64 {
            return None;
        }
        return self.hndl_to_descs[hndl as usize].as_mut();
    }
    pub fn paddr_to_hndl(&self, paddr: u64) -> Option<u64> {
        let hndl = self.paddr_to_hndls.at(paddr);
        if hndl.is_empty() {
            return None;
        } else {
            return Some(*hndl[0]);
        }
    }
    pub fn paddr_range_to_hndl(&self, paddr: u64, size: u64) -> Option<Vec<(u64, u64, u64)>> {
        let hndls: Vec<u64> = self.paddr_to_hndls.overlap(paddr, paddr + size - 1).iter().map(|x| **x).collect();
        if hndls.is_empty() {
            return None;
        }
        let mut ranged_hndl = Vec::with_capacity(hndls.len());
        let mut start = paddr;
        let mut remaining = size;
        for hndl in hndls {
            let desc = self.hndl_to_desc(hndl).unwrap();
            if start < desc.paddr {
                return None;
            }
            let delta = min(remaining, desc.size - (start - desc.paddr));
            ranged_hndl.push((hndl, start, delta));
            start += delta;
            remaining -= delta;
        }
        if remaining != 0 {
            return None;
        }
        return Some(ranged_hndl);
    }
}

#[cfg(test)]
mod desc_query_tests {
    use super::*;
    use defaultplugin::plugin;
    use std::path::Path;
    use test_aids::*;
    fn test_open_close_cb(path: &[&Path]) {
        let mut p = plugin();
        let mut descs = RIODescQuery::new();
        // Test single file opening and closing
        let mut hndl = descs.register_open(&mut p, &path[0].to_string_lossy(), IoMode::READ).unwrap();
        assert_eq!(descs.hndl_to_descs.len(), 1);
        assert_eq!(descs.paddr_to_hndls.size(), 1);
        assert_eq!(hndl, 0);
        let mut desc = descs.hndl_to_desc(0).unwrap();
        assert_eq!(desc.paddr, 0);
        descs.close(hndl).unwrap();
        assert!(descs.hndl_to_desc(0).is_none());
        assert_eq!(descs.paddr_to_hndls.size(), 0);

        // Now lets open 3 files
        // close the second one and re opening it and see what happens
        descs.register_open(&mut p, &path[0].to_string_lossy(), IoMode::READ).unwrap();
        hndl = descs.register_open(&mut p, &path[1].to_string_lossy(), IoMode::READ).unwrap();
        descs.register_open(&mut p, &path[2].to_string_lossy(), IoMode::READ).unwrap();
        assert_eq!(descs.hndl_to_descs.len(), 3);
        assert_eq!(descs.paddr_to_hndls.size(), 3);
        let mut new_paddr = 0;
        for i in 0..3 {
            desc = descs.hndl_to_desc(i).unwrap();
            assert_eq!(desc.paddr, new_paddr);
            new_paddr += desc.size;
        }
        //now lets close the second file and re open it, it should open in the same place
        descs.close(hndl).unwrap();
        assert!(descs.hndl_to_desc(hndl).is_none());
        assert_eq!(descs.paddr_to_hndls.size(), 2);
        descs.register_open(&mut p, &path[1].to_string_lossy(), IoMode::READ).unwrap();
        assert_eq!(descs.hndl_to_descs.len(), 3);
        assert_eq!(descs.paddr_to_hndls.size(), 3);
        let mut new_paddr = 0;
        for i in 0..3 {
            desc = descs.hndl_to_desc(i).unwrap();
            assert_eq!(desc.paddr, new_paddr);
            new_paddr += desc.size;
        }
        descs.close(0).unwrap();
        descs.close(1).unwrap();
        descs.close(2).unwrap();
        assert_eq!(descs.free_hndls.len(), 3);
    }
    #[test]
    fn test_open_close() {
        operate_on_files(&test_open_close_cb, &[DATA, DATA, DATA]);
    }
    fn test_open_at_cb(path: &[&Path]) {
        let mut p = plugin();
        let mut descs = RIODescQuery::new();
        descs.register_open_at(&mut p, &path[0].to_string_lossy(), IoMode::READ, 0x5000).unwrap();
        assert_eq!(descs.paddr_to_hndl(0x5000).unwrap(), 0);
        descs.close(0).unwrap();
        assert!(descs.hndl_to_desc(0).is_none());

        // now lets open 3 files where each one has paddr < the one that comes firt
        descs.register_open_at(&mut p, &path[0].to_string_lossy(), IoMode::READ, 0x5000).unwrap();
        descs.register_open_at(&mut p, &path[1].to_string_lossy(), IoMode::READ, 0x5000 - DATA.len() as u64).unwrap();
        descs.register_open(&mut p, &path[2].to_string_lossy(), IoMode::READ).unwrap();
        assert_eq!(descs.hndl_to_descs.len(), 3);
        assert_eq!(descs.paddr_to_hndls.size(), 3);
        assert_eq!(descs.hndl_to_desc(0).as_ref().unwrap().paddr, 0x5000);
        assert_eq!(descs.hndl_to_desc(1).as_ref().unwrap().paddr, 0x5000 - DATA.len() as u64);
        assert_eq!(descs.hndl_to_desc(2).as_ref().unwrap().paddr, 0);
        //now lets the middle one and re-open it
        descs.close(1).unwrap();
        descs.register_open_at(&mut p, &path[1].to_string_lossy(), IoMode::READ, 0x5000 - DATA.len() as u64).unwrap();

        assert_eq!(descs.hndl_to_descs.len(), 3);
        assert_eq!(descs.paddr_to_hndls.size(), 3);
        assert_eq!(descs.hndl_to_desc(0).as_ref().unwrap().paddr, 0x5000);
        assert_eq!(descs.hndl_to_desc(1).as_ref().unwrap().paddr, 0x5000 - DATA.len() as u64);
        assert_eq!(descs.hndl_to_desc(2).as_ref().unwrap().paddr, 0);
    }
    #[test]
    fn test_open_at() {
        operate_on_files(&test_open_at_cb, &[DATA, DATA, DATA]);
    }

    fn test_failing_open_cb(path: &[&Path]) {
        let mut p = plugin();
        let mut descs = RIODescQuery::new();
        descs.register_open(&mut p, &path[0].to_string_lossy(), IoMode::READ).unwrap();
        let mut e = descs.register_open_at(&mut p, &path[1].to_string_lossy(), IoMode::READ, 0).err().unwrap();
        assert_eq!(e, IoError::AddressesOverlapError);
        descs.register_open(&mut p, &path[1].to_string_lossy(), IoMode::READ).unwrap();
        e = descs.register_open_at(&mut p, &path[1].to_string_lossy(), IoMode::READ, DATA.len() as u64).err().unwrap();
        assert_eq!(e, IoError::AddressesOverlapError);
        e = descs.close(5).err().unwrap();
        assert_eq!(e, IoError::HndlNotFoundError);
    }
    #[test]
    fn test_failing_open() {
        operate_on_files(&test_failing_open_cb, &[DATA, DATA]);
    }

    fn test_lookups_cb(paths: &[&Path]) {
        let mut p = plugin();
        let mut descs = RIODescQuery::new();
        descs.register_open(&mut p, &paths[0].to_string_lossy(), IoMode::READ).unwrap();
        descs.register_open_at(&mut p, &paths[1].to_string_lossy(), IoMode::READ, 0x2000).unwrap();
        descs.register_open_at(&mut p, &paths[2].to_string_lossy(), IoMode::READ, 0x1000).unwrap();
        assert_eq!(descs.paddr_to_hndl(0x10).unwrap(), 0);
        assert_eq!(descs.paddr_to_hndl(0x2000).unwrap(), 1);
        assert_eq!(descs.paddr_to_hndl(0x1000).unwrap(), 2);
        assert_eq!(descs.paddr_to_hndl(0x500), None);
        assert_eq!(descs.hndl_to_desc(0).unwrap().hndl, 0);
        assert_eq!(descs.hndl_to_desc(1).unwrap().hndl, 1);
        assert_eq!(descs.hndl_to_desc(2).unwrap().hndl, 2);
        assert!(descs.hndl_to_desc(3).is_none());
        assert_eq!(descs.hndl_to_mut_desc(0).unwrap().hndl, 0);
        assert_eq!(descs.hndl_to_mut_desc(1).unwrap().hndl, 1);
        assert_eq!(descs.hndl_to_mut_desc(2).unwrap().hndl, 2);
        assert!(descs.hndl_to_mut_desc(3).is_none());
    }

    #[test]
    fn test_lookups() {
        operate_on_files(&test_lookups_cb, &[DATA, DATA, DATA]);
    }

    fn paddr_range_to_hndl_cb(paths: &[&Path]) {
        let mut p = plugin();
        let mut descs = RIODescQuery::new();
        for i in 0..3 {
            descs.register_open(&mut p, &paths[i].to_string_lossy(), IoMode::READ).unwrap();
        }
        descs.register_open_at(&mut p, &paths[3].to_string_lossy(), IoMode::READ, DATA.len() as u64 * 4).unwrap();

        assert_eq!(descs.paddr_range_to_hndl(0, 315).unwrap(), vec![(0, 0, 105), (1, 105, 105), (2, 210, 105)]);
        // overflow to the left
        assert_eq!(descs.paddr_range_to_hndl(330, 200), None);
        // overflow to the right
        assert_eq!(descs.paddr_range_to_hndl(20, 315), None);
        // overflow in the middle
        assert_eq!(descs.paddr_range_to_hndl(20, 500), None);
        // read from the middle of a descriptor
        assert_eq!(descs.paddr_range_to_hndl(20, 295).unwrap(), vec![(0, 20, 85), (1, 105, 105), (2, 210, 105)]);
        // read till the middle of descriptor
        assert_eq!(descs.paddr_range_to_hndl(0, 295).unwrap(), vec![(0, 0, 105), (1, 105, 105), (2, 210, 85)]);
        // read 1 part of a descriptor
        assert_eq!(descs.paddr_range_to_hndl(425, 75).unwrap(), vec![(3, 425, 75)]);
    }

    #[test]
    fn test_paddr_range_to_hndl() {
        operate_on_files(&paddr_range_to_hndl_cb, &[DATA, DATA, DATA, DATA]);
    }
}
