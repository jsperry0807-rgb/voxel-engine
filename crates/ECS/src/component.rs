//! Component registration metadata and the [`Bundle`] trait.

use core::alloc::Layout;
use core::any::TypeId;
use core::mem::needs_drop;

use crate::prelude::*;
use crate::world::World;

/// A compact identifier assigned to a registered component type.
///
/// `ComponentId(0)` is reserved for the synthetic `Entity` yield term used by
/// duplicate detection. Real components start at `1`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ComponentId(pub(crate) u32);

impl ComponentId {
    /// Reserved identifier used to detect `(Entity, Entity)` style queries.
    pub(crate) const ENTITY: Self = Self(0);
}

impl core::fmt::Debug for ComponentId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        if *self == Self::ENTITY {
            f.write_str("ComponentId(Entity)")
        } else {
            write!(f, "ComponentId({})", self.0)
        }
    }
}

/// Where a component type is stored.
///
/// The choice is made once, at [`World::register`](crate::World::register), and
/// is permanent for the lifetime of the world.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Storage {
    /// Columnar storage. Adding or removing a table component migrates the
    /// entity to another archetype/table.
    Table,
    /// Sparse-set storage. Adding or removing a sparse component never
    /// migrates the entity and never affects archetype identity.
    SparseSet,
}

#[allow(dead_code)]
pub(crate) struct ComponentInfo {
    pub id: ComponentId,
    pub type_id: TypeId,
    pub name: &'static str,
    pub layout: Layout,
    pub drop_fn: Option<unsafe fn(*mut u8)>,
    pub storage: Storage,
    pub is_zst: bool,
}

/// Type-erased drop glue for `T`.
///
/// # Safety
///
/// `ptr` must point to a valid, properly aligned, initialized `T` that is not
/// used again after this call.
unsafe fn drop_component<T>(ptr: *mut u8) {
    // SAFETY: guaranteed by the caller; `drop_in_place` requires a valid,
    // aligned, initialized value.
    unsafe { core::ptr::drop_in_place(ptr.cast::<T>()) }
}

/// Registry of component types.
pub(crate) struct Registry {
    infos: Vec<ComponentInfo>,
    by_type: FxHashMap<TypeId, ComponentId>,
}

impl Registry {
    pub(crate) fn new() -> Self {
        let entity = ComponentInfo {
            id: ComponentId::ENTITY,
            type_id: TypeId::of::<()>(),
            name: "<entity>",
            layout: Layout::new::<()>(),
            drop_fn: None,
            storage: Storage::Table,
            is_zst: true,
        };
        Self {
            infos: vec![entity],
            by_type: FxHashMap::default(),
        }
    }

    /// Number of registered component ids, including the reserved `Entity`
    /// pseudo-id at index 0.
    pub(crate) fn len(&self) -> usize {
        self.infos.len()
    }

    pub(crate) fn info(&self, id: ComponentId) -> &ComponentInfo {
        &self.infos[id.0 as usize]
    }

    pub(crate) fn storage(&self, id: ComponentId) -> Storage {
        self.info(id).storage
    }

    pub(crate) fn id_of<T: Send + 'static>(&self) -> Option<ComponentId> {
        self.by_type.get(&TypeId::of::<T>()).copied()
    }

    /// Returns the id of `T`, panicking with the frozen prefix if the type was
    /// never registered.
    pub(crate) fn require<T: Send + 'static>(&self) -> ComponentId {
        match self.id_of::<T>() {
            Some(id) => id,
            None => panic!(
                "ecs: component not registered: `{}` must be registered with World::register",
                core::any::type_name::<T>()
            ),
        }
    }

    /// Registers `T` with the requested storage. Returns the component id and
    /// whether this call created a new registration.
    pub(crate) fn register<T: Send + 'static>(&mut self, storage: Storage) -> (ComponentId, bool) {
        let type_id = TypeId::of::<T>();
        if let Some(&id) = self.by_type.get(&type_id) {
            let existing = self.infos[id.0 as usize].storage;
            if existing != storage {
                panic!(
                    "ecs: storage mismatch: `{}` was registered as {:?}, not {:?}",
                    core::any::type_name::<T>(),
                    existing,
                    storage
                );
            }
            return (id, false);
        }

        if self.infos.len() >= u32::MAX as usize {
            panic!(
                "ecs: component id exhausted: {} component types registered",
                u32::MAX
            );
        }

        let id = ComponentId(self.infos.len() as u32);
        let layout = Layout::new::<T>();
        let drop_fn = if needs_drop::<T>() {
            Some(drop_component::<T> as unsafe fn(*mut u8))
        } else {
            None
        };
        self.infos.push(ComponentInfo {
            id,
            type_id,
            name: core::any::type_name::<T>(),
            layout,
            drop_fn,
            storage,
            is_zst: layout.size() == 0,
        });
        self.by_type.insert(type_id, id);
        (id, true)
    }
}

mod sealed {
    pub trait BundleSealed {}
}

/// A statically known group of components.
///
/// Implemented for `()` and tuples of one through sixteen components whose
/// element types are `Send + 'static`. The trait is sealed: `Box<dyn Bundle>`
/// is intentionally not part of the 1.0 surface.
///
/// # Panics
///
/// Spawning or inserting a bundle panics with
/// `"ecs: duplicate component in bundle:"` if the same component type appears
/// twice, and with `"ecs: component not registered:"` if any element was never
/// registered.
pub trait Bundle: BundleImpl {}

/// Hidden implementation detail of [`Bundle`]. Sealed.
#[doc(hidden)]
pub trait BundleImpl: Send + sealed::BundleSealed + 'static {
    /// Collects the component ids of this bundle, in tuple order.
    fn component_ids(world: &World) -> SmallVec<[ComponentId; 16]>
    where
        Self: Sized;

    /// Moves each component of the bundle into `world` for entity `e`.
    ///
    /// The entity must have been freshly allocated (or freshly migrated) so
    /// that every component slot is uninitialized.
    fn put(self, world: &mut World, e: crate::Entity)
    where
        Self: Sized;
}

impl<B: BundleImpl> Bundle for B {}

macro_rules! impl_bundle {
    ($($name:ident),+) => {
        impl<$($name: Send + 'static),+> sealed::BundleSealed for ($($name,)+) {}

        impl<$($name: Send + 'static),+> BundleImpl for ($($name,)+) {
            fn component_ids(world: &World) -> SmallVec<[ComponentId; 16]> {
                let mut ids: SmallVec<[ComponentId; 16]> = SmallVec::new();
                $( ids.push(world.require_component::<$name>()); )+
                ids
            }

            fn put(self, world: &mut World, e: crate::Entity) {
                #[allow(non_snake_case)]
                let ($($name,)+) = self;
                $( world.put_new::<$name>(e, $name); )+
            }
        }
    };
}

impl sealed::BundleSealed for () {}

impl BundleImpl for () {
    fn component_ids(_world: &World) -> SmallVec<[ComponentId; 16]> {
        SmallVec::new()
    }

    fn put(self, _world: &mut World, _e: crate::Entity) {}
}

impl_bundle!(A);
impl_bundle!(A, B);
impl_bundle!(A, B, C);
impl_bundle!(A, B, C, D);
impl_bundle!(A, B, C, D, E);
impl_bundle!(A, B, C, D, E, F);
impl_bundle!(A, B, C, D, E, F, G);
impl_bundle!(A, B, C, D, E, F, G, H);
impl_bundle!(A, B, C, D, E, F, G, H, I);
impl_bundle!(A, B, C, D, E, F, G, H, I, J);
impl_bundle!(A, B, C, D, E, F, G, H, I, J, K);
impl_bundle!(A, B, C, D, E, F, G, H, I, J, K, L);
impl_bundle!(A, B, C, D, E, F, G, H, I, J, K, L, M);
impl_bundle!(A, B, C, D, E, F, G, H, I, J, K, L, M, N);
impl_bundle!(A, B, C, D, E, F, G, H, I, J, K, L, M, N, O);
impl_bundle!(A, B, C, D, E, F, G, H, I, J, K, L, M, N, O, P);
