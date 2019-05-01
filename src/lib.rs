#![feature(alloc_layout_extra)]
#![feature(test)]

use std::alloc::{alloc_zeroed, dealloc, handle_alloc_error, Layout};
use std::marker::PhantomData;
use std::ptr::NonNull;

const EMPTY: u8 = 0;
const TAKEN: u8 = 1;

struct Slot<T> {
    flag: u8, // не самое оптимальное решение по памяти из-за выравнивания структуры
    key: usize,
    value: T,
}

/// простейшая хэш-таблица, ключем которой является `usize` значение, хэш-функция от ключа KEY % MAP_CAPACITY
pub struct HashMap<V> {
    slots: NonNull<Slot<V>>,
    items: usize,
    capacity: usize,
    marker: PhantomData<V>,
}

impl<V> HashMap<V> {
    pub fn new() -> HashMap<V> {
        HashMap {
            slots: NonNull::dangling(),
            items: 0,
            capacity: 0,
            marker: PhantomData,
        }
    }

    unsafe fn new_inner(capacity: usize) -> HashMap<V> {
        let capacity = capacity.next_power_of_two();
        let layout = Layout::array::<Slot<V>>(capacity).unwrap();
        let slots = alloc_zeroed(layout) as *mut Slot<V>;

        if slots.is_null() {
            handle_alloc_error(layout);
        }

        HashMap {
            slots: NonNull::new_unchecked(slots),
            capacity,
            items: 0,
            marker: PhantomData,
        }
    }

    pub fn with_capacity(capacity: usize) -> HashMap<V> {
        unsafe { Self::new_inner(capacity) }
    }

    // максимально простое линейное пробирование с шагом в единицу
    fn prob_seq(&self, hash: usize) -> impl Iterator<Item = usize> {
        let capacity = self.capacity;
        (0..capacity).map(move |idx| (hash + idx) % capacity)
    }

    fn find(&self, key: usize) -> Option<&mut Slot<V>> {
        if self.capacity == 0 {
            return None;
        }

        let hash = key % self.capacity;
        let slots = self.slots.as_ptr();

        for idx in self.prob_seq(hash) {
            let slot = unsafe { &mut *slots.add(idx) };

            if slot.flag == EMPTY {
                return None;
            }

            if slot.flag == TAKEN && slot.key == key {
                return Some(slot);
            }
        }

        None
    }

    fn find_insert_slot(&self, hash: usize) -> usize {
        let slots = self.slots.as_ptr();

        for idx in self.prob_seq(hash) {
            let slot = unsafe { &*slots.add(idx) };

            if slot.flag == EMPTY {
                return idx;
            }
        }

        unreachable!();
    }

    pub fn get<'a>(&'a self, key: usize) -> Option<&'a V> {
        self.find(key).map(|slot| &slot.value)
    }

    pub fn get_mut<'a>(&'a mut self, key: usize) -> Option<&'a mut V> {
        self.find(key).map(|slot| &mut slot.value)
    }

    pub fn insert(&mut self, key: usize, value: V) -> Option<V> {
        if let Some(slot) = self.find(key) {
            Some(std::mem::replace(&mut slot.value, value))
        } else {
            self.reserve(1);
            self.insert_inner(key, value);
            None
        }
    }

    fn insert_inner(&mut self, key: usize, value: V) {
        let hash = key % self.capacity;
        let index = self.find_insert_slot(hash);

        let slot = Slot {
            flag: TAKEN,
            key,
            value,
        };

        unsafe {
            self.slots.as_ptr().add(index).write(slot);
        }

        self.items += 1;
    }

    pub fn remove(&mut self, key: usize) -> Option<V> {
        let value = self.find(key).map(|slot| unsafe {
            slot.flag = EMPTY;
            std::mem::replace(&mut slot.value, std::mem::zeroed())
        })?;

        self.items -= 1;

        Some(value)
    }

    pub fn reserve(&mut self, additional: usize) {
        if additional + self.items > self.capacity {
            self.resize(additional + self.items);
        }
    }

    pub fn resize(&mut self, new_size: usize) {
        assert!(
            new_size >= self.items,
            "the new size is less than count of items"
        );

        unsafe {
            let mut map = Self::new_inner(new_size);
            let slots = self.slots.as_ptr();

            for idx in 0..self.capacity {
                let mut slot = &mut *slots.add(idx);

                if slot.flag == TAKEN {
                    let hash = slot.key % map.capacity();
                    let index = map.find_insert_slot(hash);
                    std::mem::swap(&mut *map.slots.as_ptr().add(index), &mut slot);
                    map.items += 1;
                }
            }

            std::mem::swap(self, &mut map);
        }
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn len(&self) -> usize {
        self.items
    }
}

impl<V> Drop for HashMap<V> {
    fn drop(&mut self) {
        if self.capacity != 0 {
            unsafe {
                let layout = Layout::array::<Slot<V>>(self.capacity).unwrap();
                let slots = self.slots.as_ptr();

                if std::mem::needs_drop::<V>() {
                    for idx in 0..self.capacity {
                        let slot = &*slots.add(idx);
                        if slot.flag == TAKEN {
                            slots.add(idx).drop_in_place();
                        }
                    }
                }

                dealloc(slots as *mut u8, layout);
            }
        }
    }
}

unsafe impl<V: Send> Send for HashMap<V> {}

#[cfg(test)]
mod tests {
    extern crate test;

    use super::HashMap;
    use rand::random;
    use std::collections::HashMap as StdMap;
    use test::Bencher;

    #[test]
    fn empty_hashmap() {
        let hashmap: HashMap<f32> = HashMap::new();
        assert_eq!(hashmap.capacity(), 0);
        assert_eq!(hashmap.get(0), None);
    }

    #[test]
    fn resize() {
        let mut hashmap: HashMap<f32> = HashMap::with_capacity(1);
        hashmap.insert(0, 0.1);
        hashmap.insert(1, 0.2);
        assert_eq!(hashmap.get(0).copied(), Some(0.1));
        assert_eq!(hashmap.get(1).copied(), Some(0.2));
    }

    #[test]
    fn capacity() {
        let mut hashmap: HashMap<f32> = HashMap::with_capacity(12);
        assert_eq!(hashmap.capacity(), 16);
        hashmap.insert(15, 0.21);
        assert_eq!(hashmap.capacity(), 16);
    }

    #[test]
    fn collision() {
        let mut hashmap: HashMap<f32> = HashMap::with_capacity(2);
        hashmap.insert(2, 0.1); // 2 % 2 == 0
        hashmap.insert(4, 0.2); // 4 % 2 == 0

        assert_eq!(hashmap.get(2).copied(), Some(0.1));
        assert_eq!(hashmap.get(4).copied(), Some(0.2));
    }

    #[test]
    fn double_insert() {
        let mut hashmap: HashMap<f32> = HashMap::new();
        hashmap.insert(10, 0.1);
        assert_eq!(hashmap.insert(10, 0.2), Some(0.1));
        assert_eq!(hashmap.get(10).copied(), Some(0.2));
        assert_eq!(hashmap.len(), 1);
    }

    #[test]
    fn dont_die_please() {
        let mut hashmap: HashMap<f32> = HashMap::new();
        let mut array = vec![];

        for key in 0..1_000_000 {
            let value = random();
            hashmap.insert(key, value);
            array.push(value);
        }

        for (key, value) in array.iter().enumerate() {
            assert_eq!(hashmap.get(key), Some(value));
        }
    }

    #[bench]
    fn my_hashmap(b: &mut Bencher) {
        let mut hashmap: HashMap<u64> = HashMap::new();

        b.iter(|| {
            for key in 0..500_000 {
                let value = random();
                hashmap.insert(key, value);
            }
        });
    }

    #[bench]
    fn std_hashmap(b: &mut Bencher) {
        let mut stdmap: StdMap<usize, u64> = StdMap::new();

        b.iter(|| {
            for key in 0..500_000 {
                let value = random();
                stdmap.insert(key, value);
            }
        });
    }
}
