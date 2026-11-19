#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0
"""Host regression test of the actual kernel Vec::swap_remove method.

Only allocation/storage is supplied by std. Extract the production method
unchanged, so the test checks the kernel implementation rather than a copy.
Invalid-index tests deny pointer access in the storage adapter: a regressed
check fails safely before any invalid read, including for an empty vector.
"""
from pathlib import Path
import os
import shutil
import subprocess
import tempfile

root = Path(__file__).resolve().parents[4]
rustc = os.environ.get("RUSTC", "rustc")
if not shutil.which(rustc):
    print("# SKIP: rustc is required")
    raise SystemExit(4)
source = (root / "rust/kernel/alloc/kvec.rs").read_text()
start = source.index("    pub fn swap_remove(&mut self, index: usize) -> T {")
end = source.index("\n    }", start) + len("\n    }")
method = source[start:end]
harness = r'''
use std::{cell::RefCell, panic::{catch_unwind, AssertUnwindSafe}, ptr, rc::Rc};
struct KernelVec<T> { storage: Vec<T>, deny_pointer_access: bool }
impl<T> KernelVec<T> {
    fn new(storage: Vec<T>) -> Self { Self { storage, deny_pointer_access: false } }
    fn len(&self) -> usize { self.storage.len() }
    fn as_ptr(&self) -> *const T {
        assert!(!self.deny_pointer_access, "invalid index reached pointer access");
        self.storage.as_ptr()
    }
    fn as_mut_ptr(&mut self) -> *mut T {
        assert!(!self.deny_pointer_access, "invalid index reached pointer access");
        self.storage.as_mut_ptr()
    }
    unsafe fn dec_len(&mut self, n: usize) {
        unsafe { self.storage.set_len(self.len() - n) }
    }
    /* PRODUCTION_METHOD */
}
fn rejects(v: Vec<i32>, index: usize) {
    let mut v = KernelVec::new(v);
    let before = v.storage.clone();
    v.deny_pointer_access = true;
    let panic = catch_unwind(AssertUnwindSafe(|| v.swap_remove(index)))
        .expect_err("invalid index was accepted");
    let message = panic.downcast_ref::<&str>().copied()
        .or_else(|| panic.downcast_ref::<String>().map(String::as_str));
    assert_eq!(message, Some("Index out of range"), "must reject before unsafe access");
    assert_eq!(v.storage, before);
}
#[test] fn empty_equal_bound() { rejects(vec![], 0); }
#[test] fn nonempty_equal_bound() { rejects(vec![10, 20], 2); }
#[test] fn greater_than_bound() { rejects(vec![10, 20], 3); rejects(vec![], usize::MAX); }
#[test] fn valid_indices() {
    for index in 0..4 {
        let mut actual = KernelVec::new(vec![10, 20, 30, 40]);
        let mut expected = actual.storage.clone();
        assert_eq!(actual.swap_remove(index), expected.swap_remove(index));
        assert_eq!(actual.storage, expected);
    }
    let mut v = KernelVec::new(vec![42]);
    assert_eq!(v.swap_remove(0), 42);
    assert!(v.storage.is_empty());
}
#[test] fn each_value_dropped_once() {
    struct D(usize, Rc<RefCell<Vec<usize>>>);
    impl Drop for D { fn drop(&mut self) { self.1.borrow_mut().push(self.0); } }
    let drops = Rc::new(RefCell::new(vec![]));
    let mut v = KernelVec::new((0..4).map(|i| D(i, drops.clone())).collect());
    let removed = v.swap_remove(1);
    assert_eq!(removed.0, 1);
    assert!(drops.borrow().is_empty());
    drop(removed); drop(v);
    let mut got = drops.borrow().clone(); got.sort();
    assert_eq!(got, vec![0, 1, 2, 3]);
}
#[test] fn zero_sized_elements() {
    let mut v = KernelVec::new(vec![(); 3]);
    v.swap_remove(1); assert_eq!(v.len(), 2);
}
'''.replace("    /* PRODUCTION_METHOD */", method)
with tempfile.TemporaryDirectory(prefix="kernel-kvec-") as scratch:
    scratch = Path(scratch)
    test = scratch / "swap_remove.rs"
    binary = scratch / "swap_remove"
    test.write_text(harness)
    subprocess.run([rustc, "--edition=2021", "--test", str(test), "-o", str(binary)], check=True)
    subprocess.run([str(binary)], check=True)
