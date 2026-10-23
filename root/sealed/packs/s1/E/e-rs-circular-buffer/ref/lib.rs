// Reference solution of e-rs-circular-buffer (rendered by gen/run with the seeded constants).
use std::collections::VecDeque;

const READ_NEWEST: bool = @READ_NEWEST@;
const EVICT_NEWEST: bool = @EVICT_NEWEST@;

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    EmptyBuffer,
    FullBuffer,
}

/// A bounded buffer; the front of `items` is the oldest element.
pub struct CircularBuffer<T> {
    items: VecDeque<T>,
    capacity: usize,
}

impl<T> CircularBuffer<T> {
    pub fn new(capacity: usize) -> Self {
        CircularBuffer { items: VecDeque::with_capacity(capacity), capacity }
    }

    pub fn write(&mut self, element: T) -> Result<(), Error> {
        if self.items.len() >= self.capacity {
            return Err(Error::FullBuffer);
        }
        self.items.push_back(element);
        Ok(())
    }

    pub fn read(&mut self) -> Result<T, Error> {
        let x = if READ_NEWEST { self.items.pop_back() } else { self.items.pop_front() };
        x.ok_or(Error::EmptyBuffer)
    }

    pub fn clear(&mut self) {
        self.items.clear();
    }

    pub fn overwrite(&mut self, element: T) {
        if self.capacity == 0 {
            return;
        }
        if self.items.len() >= self.capacity {
            if EVICT_NEWEST {
                self.items.pop_back();
            } else {
                self.items.pop_front();
            }
        }
        self.items.push_back(element);
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn peek(&self) -> Option<&T> {
        if READ_NEWEST { self.items.back() } else { self.items.front() }
    }

    pub fn drain(&mut self) -> Vec<T> {
        let mut out = Vec::with_capacity(self.items.len());
        while let Ok(x) = self.read() {
            out.push(x);
        }
        out
    }
}
