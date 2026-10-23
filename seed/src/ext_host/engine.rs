//! Engine `Config`, pooling limits, per-store `StoreLimits` and the in-memory Cranelift
//! incremental cache (ext/host.md §3.3, §3.4, §3.14).

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex};

use wasmtime::{
    CacheStore, Config, Engine, InstanceAllocationStrategy, OptLevel, PoolingAllocationConfig,
    ResourceLimiter, StoreLimits, StoreLimitsBuilder, Strategy,
};

use super::{INCR_CACHE_BYTES, MAX_MEMORY_BYTES, TABLE_ELEMENTS};

/// §3.3. Cranelift only: the `winch` feature is not compiled in, so `Strategy::Winch` cannot be
/// selected at all (CVE-2026-34987).
pub(crate) fn new_engine(incr: Arc<MemCacheStore>) -> wasmtime::Result<Engine> {
    let mut cfg = Config::new();
    cfg.strategy(Strategy::Cranelift)
        .cranelift_opt_level(OptLevel::Speed)
        .wasm_component_model(true)
        .epoch_interruption(true)
        .consume_fuel(false)
        .allocation_strategy(InstanceAllocationStrategy::Pooling(pool_config()))
        .memory_reservation(4 << 30)
        .memory_guard_size(32 << 20)
        .memory_init_cow(true)
        .max_wasm_stack(512 << 10)
        .parallel_compilation(true)
        .concurrency_support(false)
        .wasm_backtrace_max_frames(NonZeroUsize::new(20))
        .macos_use_mach_ports(true)
        .signals_based_traps(true);
    cfg.enable_incremental_compilation(incr)?;
    Engine::new(&cfg)
}

/// §3.4: small pool totals; one component instance per in-flight call.
pub(crate) fn pool_config() -> PoolingAllocationConfig {
    let mut p = PoolingAllocationConfig::new();
    p.total_component_instances(32)
        .total_core_instances(256)
        .total_memories(32)
        .total_tables(128)
        .total_stacks(32)
        .max_component_instance_size(1 << 20)
        .max_core_instance_size(1 << 20)
        .max_core_instances_per_component(16)
        .max_memories_per_component(2)
        .max_tables_per_component(8)
        .max_memories_per_module(1)
        .max_tables_per_module(2)
        .table_elements(TABLE_ELEMENTS)
        .max_memory_size(MAX_MEMORY_BYTES);
    p
}

/// `StoreLimits` plus a flag that records a denied `memory.grow`, so the result can say
/// "out of memory" rather than "panicked" (§3.4).
pub(crate) struct CallLimiter {
    inner: StoreLimits,
    pub memory_denied: bool,
}

impl CallLimiter {
    pub(crate) fn new() -> Self {
        let inner = StoreLimitsBuilder::new()
            .memory_size(MAX_MEMORY_BYTES)
            .table_elements(TABLE_ELEMENTS)
            .instances(16)
            .tables(16)
            .memories(2)
            .trap_on_grow_failure(false)
            .build();
        CallLimiter {
            inner,
            memory_denied: false,
        }
    }
}

impl ResourceLimiter for CallLimiter {
    fn memory_growing(
        &mut self,
        current: usize,
        desired: usize,
        maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        let ok = self.inner.memory_growing(current, desired, maximum)?;
        if !ok {
            self.memory_denied = true;
        }
        Ok(ok)
    }

    fn memory_grow_failed(&mut self, _error: wasmtime::Error) -> wasmtime::Result<()> {
        // Growth past the pool's or the memory type's maximum: the same "out of memory" for us.
        self.memory_denied = true;
        Ok(())
    }

    fn table_growing(
        &mut self,
        current: usize,
        desired: usize,
        maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        self.inner.table_growing(current, desired, maximum)
    }

    fn instances(&self) -> usize {
        self.inner.instances()
    }

    fn tables(&self) -> usize {
        self.inner.tables()
    }

    fn memories(&self) -> usize {
        self.inner.memories()
    }
}

/// §3.14 option D: Cranelift's incremental cache, in our own memory only. No disk cache, no
/// `deserialize`: nothing outside this process can put native code into it.
/// (key → value, total bytes)
type CacheMap = (BTreeMap<Vec<u8>, Vec<u8>>, usize);

#[derive(Debug, Default)]
pub(crate) struct MemCacheStore {
    inner: Mutex<CacheMap>,
}

impl MemCacheStore {
    /// (entries, bytes), for measurements.
    pub(crate) fn stats(&self) -> (usize, usize) {
        self.inner
            .lock()
            .map(|g| (g.0.len(), g.1))
            .unwrap_or((0, 0))
    }
}

impl CacheStore for MemCacheStore {
    fn get(&self, key: &[u8]) -> Option<Cow<'_, [u8]>> {
        let g = self.inner.lock().ok()?;
        g.0.get(key).map(|v| Cow::Owned(v.clone()))
    }

    fn insert(&self, key: &[u8], value: Vec<u8>) -> bool {
        let Ok(mut g) = self.inner.lock() else {
            return false;
        };
        let add = key.len() + value.len();
        if g.1 + add > INCR_CACHE_BYTES {
            // Simplest eviction: start over.
            g.0.clear();
            g.1 = 0;
        }
        g.1 += add;
        g.0.insert(key.to_vec(), value);
        true
    }
}
