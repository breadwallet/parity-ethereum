// Copyright 2015-2018 Parity Technologies (UK) Ltd.
// This file is part of Parity.

// Parity is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Parity is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Parity.  If not, see <http://www.gnu.org/licenses/>.

//! Reference-counted memory-based `HashDB` implementation.
extern crate elastic_array;
extern crate hashdb;
extern crate heapsize;
extern crate plain_hasher;
extern crate rlp;
#[cfg(test)] extern crate tiny_keccak;
#[cfg(test)] extern crate ethereum_types;

use hashdb::{HashDB, Hasher, DBValue, KeccakHasher};
use heapsize::HeapSizeOf;
use plain_hasher::H256FastMap;
use rlp::NULL_RLP;
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::mem;

/// Reference-counted memory-based `HashDB` implementation.
///
/// Use `new()` to create a new database. Insert items with `insert()`, remove items
/// with `remove()`, check for existence with `contains()` and lookup a hash to derive
/// the data with `get()`. Clear with `clear()` and purge the portions of the data
/// that have no references with `purge()`.
///
/// # Example
/// ```rust
/// extern crate hashdb;
/// extern crate memorydb;
/// use hashdb::*;
/// use memorydb::*;
/// fn main() {
///   let mut m = MemoryDB::<KeccakHasher>::new();
///   let d = "Hello world!".as_bytes();
///
///   let k = m.insert(d);
///   assert!(m.contains(&k));
///   assert_eq!(m.get(&k).unwrap(), d);
///
///   m.insert(d);
///   assert!(m.contains(&k));
///
///   m.remove(&k);
///   assert!(m.contains(&k));
///
///   m.remove(&k);
///   assert!(!m.contains(&k));
///
///   m.remove(&k);
///   assert!(!m.contains(&k));
///
///   m.insert(d);
///   assert!(!m.contains(&k));

///   m.insert(d);
///   assert!(m.contains(&k));
///   assert_eq!(m.get(&k).unwrap(), d);
///
///   m.remove(&k);
///   assert!(!m.contains(&k));
/// }
/// ```
#[derive(Default, Clone, PartialEq)]
pub struct MemoryDB<H: Hasher> {
	data: H256FastMap<H, (DBValue, i32)>,
}

/// Convenience type for crates that need a `MemoryDB` with Keccak hashes
pub type KeccakMemoryDB = MemoryDB<KeccakHasher>;

impl<H: Hasher> MemoryDB<H> {
	/// Create a new instance of the memory DB.
	pub fn new() -> MemoryDB<H> {
		MemoryDB {
			data: H256FastMap::<H,_>::default()
		}
	}

	/// Clear all data from the database.
	///
	/// # Examples
	/// ```rust
	/// extern crate hashdb;
	/// extern crate memorydb;
	/// use hashdb::*;
	/// use memorydb::*;
	/// fn main() {
	///   let mut m = MemoryDB::<KeccakHasher>::new();
	///   let hello_bytes = "Hello world!".as_bytes();
	///   let hash = m.insert(hello_bytes);
	///   assert!(m.contains(&hash));
	///   m.clear();
	///   assert!(!m.contains(&hash));
	/// }
	/// ```
	pub fn clear(&mut self) {
		self.data.clear();
	}

	/// Purge all zero-referenced data from the database.
	pub fn purge(&mut self) {
		self.data.retain(|_, &mut (_, rc)| rc != 0);
	}

	/// Return the internal map of hashes to data, clearing the current state.
	pub fn drain(&mut self) -> H256FastMap<H, (DBValue, i32)> {
		mem::replace(&mut self.data, H256FastMap::<H,_>::default())
	}

	/// Grab the raw information associated with a key. Returns None if the key
	/// doesn't exist.
	///
	/// Even when Some is returned, the data is only guaranteed to be useful
	/// when the refs > 0.
	pub fn raw(&self, key: &<H as Hasher>::Out) -> Option<(DBValue, i32)> {
		if key == &H::HASHED_NULL_RLP {
			return Some((DBValue::from_slice(&NULL_RLP), 1));
		}
		self.data.get(key).cloned()
	}

	/// Returns the size of allocated heap memory
	pub fn mem_used(&self) -> usize {
		self.data.heap_size_of_children()
	}

	/// Remove an element and delete it from storage if reference count reaches zero.
	/// If the value was purged, return the old value.
	pub fn remove_and_purge(&mut self, key: &<H as Hasher>::Out) -> Option<DBValue> {
		if key == &H::HASHED_NULL_RLP {
			return None;
		}
		match self.data.entry(key.clone()) {
			Entry::Occupied(mut entry) =>
				if entry.get().1 == 1 {
					Some(entry.remove().0)
				} else {
					entry.get_mut().1 -= 1;
					None
				},
			Entry::Vacant(entry) => {
				entry.insert((DBValue::new(), -1));
				None
			}
		}
	}

	/// Consolidate all the entries of `other` into `self`.
	pub fn consolidate(&mut self, mut other: Self) {
		for (key, (value, rc)) in other.drain() {
			match self.data.entry(key) {
				Entry::Occupied(mut entry) => {
					if entry.get().1 < 0 {
						entry.get_mut().0 = value;
					}

					entry.get_mut().1 += rc;
				}
				Entry::Vacant(entry) => {
					entry.insert((value, rc));
				}
			}
		}
	}
}

impl<H: Hasher> HashDB for MemoryDB<H> {
	type H = H;

	// REVIEW: this method is what made it necessary to add a type param to H256FastMap, which I'd rather have avoided.
	//         The problem is that the keys returned are `H256` and type inference fails on the `collect()` call.
	//         I could not make it work without parameterizing H256FastMap too. It all sort of adds up as I could
	//         avoid adding PhantomData to MemoryDB, but still quite annoying. What's a better way?
	fn keys(&self) -> HashMap<<Self::H as Hasher>::Out, i32> {
		self.data.iter()
			.filter_map(|(k, v)| if v.1 != 0 {
				Some((*k, v.1))
			} else {
				None
			})
			.collect()
	}

	fn get(&self, key: &<Self::H as Hasher>::Out) -> Option<DBValue> {
		if key == &Self::H::HASHED_NULL_RLP {
			return Some(DBValue::from_slice(&NULL_RLP));
		}

		match self.data.get(key) {
			Some(&(ref d, rc)) if rc > 0 => Some(d.clone()),
			_ => None
		}
	}

	fn contains(&self, key: &<Self::H as Hasher>::Out) -> bool {
		if key == &Self::H::HASHED_NULL_RLP {
			return true;
		}

		match self.data.get(key) {
			Some(&(_, x)) if x > 0 => true,
			_ => false
		}
	}

	fn insert(&mut self, value: &[u8]) -> <Self::H as Hasher>::Out {
		if value == &NULL_RLP {
			return Self::H::HASHED_NULL_RLP.clone();
		}
		let key = Self::H::hash(value);
		match self.data.entry(key) {
			Entry::Occupied(mut entry) => {
				let &mut (ref mut old_value, ref mut rc) = entry.get_mut();
				if *rc <= 0 {
					*old_value = DBValue::from_slice(value);
				}
				*rc += 1;
			},
			Entry::Vacant(entry) => {
				entry.insert((DBValue::from_slice(value), 1));
			},
		}
		key
	}

	fn emplace(&mut self, key:<Self::H as Hasher>::Out, value: DBValue) {
		if &*value == &NULL_RLP {
			return;
		}

		match self.data.entry(key) {
			Entry::Occupied(mut entry) => {
				let &mut (ref mut old_value, ref mut rc) = entry.get_mut();
				if *rc <= 0 {
					*old_value = value;
				}
				*rc += 1;
			},
			Entry::Vacant(entry) => {
				entry.insert((value, 1));
			},
		}
	}

	fn remove(&mut self, key: &<Self::H as Hasher>::Out) {
		if key == &Self::H::HASHED_NULL_RLP {
			return;
		}

		match self.data.entry(*key) {
			Entry::Occupied(mut entry) => {
				let &mut (_, ref mut rc) = entry.get_mut();
				*rc -= 1;
			},
			Entry::Vacant(entry) => {
				entry.insert((DBValue::new(), -1));
			},
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use tiny_keccak::Keccak;
	use ethereum_types::H256;
	use hashdb::KeccakHasher;

	#[test]
	fn memorydb_remove_and_purge() {
		let hello_bytes = b"Hello world!";
		let mut hello_key = [0;32];
		Keccak::keccak256(hello_bytes, &mut hello_key);
		let hello_key = H256(hello_key);

		let mut m = MemoryDB::<KeccakHasher>::new();
		m.remove(&hello_key);
		assert_eq!(m.raw(&hello_key).unwrap().1, -1);
		m.purge();
		assert_eq!(m.raw(&hello_key).unwrap().1, -1);
		m.insert(hello_bytes);
		assert_eq!(m.raw(&hello_key).unwrap().1, 0);
		m.purge();
		assert_eq!(m.raw(&hello_key), None);

		let mut m = MemoryDB::<KeccakHasher>::new();
		assert!(m.remove_and_purge(&hello_key).is_none());
		assert_eq!(m.raw(&hello_key).unwrap().1, -1);
		m.insert(hello_bytes);
		m.insert(hello_bytes);
		assert_eq!(m.raw(&hello_key).unwrap().1, 1);
		assert_eq!(&*m.remove_and_purge(&hello_key).unwrap(), hello_bytes);
		assert_eq!(m.raw(&hello_key), None);
		assert!(m.remove_and_purge(&hello_key).is_none());
	}

	#[test]
	fn consolidate() {
		let mut main = MemoryDB::<KeccakHasher>::new();
		let mut other = MemoryDB::<KeccakHasher>::new();
		let remove_key = other.insert(b"doggo");
		main.remove(&remove_key);

		let insert_key = other.insert(b"arf");
		main.emplace(insert_key, DBValue::from_slice(b"arf"));

		let negative_remove_key = other.insert(b"negative");
		other.remove(&negative_remove_key);	// ref cnt: 0
		other.remove(&negative_remove_key);	// ref cnt: -1
		main.remove(&negative_remove_key);	// ref cnt: -1

		main.consolidate(other);

		let overlay = main.drain();

		assert_eq!(overlay.get(&remove_key).unwrap(), &(DBValue::from_slice(b"doggo"), 0));
		assert_eq!(overlay.get(&insert_key).unwrap(), &(DBValue::from_slice(b"arf"), 2));
		assert_eq!(overlay.get(&negative_remove_key).unwrap(), &(DBValue::from_slice(b"negative"), -2));
	}

//	#[test]
//	fn other_hashers() {
//		struct DummyHasher;
//		impl Hasher for DummyHasher {
//			type Out = ethereum_types::H160;
//			const HASHED_NULL_RLP: ethereum_types::H160= ethereum_types::H160([0; 20]);
//			fn hash(_x: &[u8]) -> Self::Out { ethereum_types::H160(*b"01010202010102020101") }
//		}
//		impl HeapSizeOf for DummyHasher { fn heap_size_of_children(&self) -> usize { 0 } }
//
//		let mut db = MemoryDB::<DummyHasher>::new();
//		// TODO: Fails. Trying to use a type that isn't H256 fails because of the tight coupling between memorydb and plain_hasher (specifically the assert on key length == 32)
//		 let key = db.insert(b"32103210321032103210321032103210");
//		 assert_eq!(key, ethereum_types::H264(*b"010102020101020201010202010102025"));
//	}
}
