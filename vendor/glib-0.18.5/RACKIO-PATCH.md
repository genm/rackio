# Rackio patch for glib 0.18.5

This is the upstream glib 0.18.5 crate, distributed under MIT. Rackio carries
one backport in src/variant_iter.rs: VariantStrIter::impl_get now passes a
mutable pointer to g_variant_get_child.

The change is the exact fix from gtk-rs/gtk-rs-core PR #1343
(https://github.com/gtk-rs/gtk-rs-core/pull/1343), merged upstream on 2024-03-30.
It prevents undefined behavior in the C out-parameter call. Keep this patch
until Rackio removes the Linux GTK 0.18 dependency family; then remove this
vendor override. The GTK4 migration and Linux desktop smoke evidence remain tracked
in https://github.com/genm/rackio/issues/14.

The upstream crate's LICENSE and copyright notices are preserved.
