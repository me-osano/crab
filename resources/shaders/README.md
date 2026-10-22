# Shaders

Custom GLSL for crab's effect passes (rounded corners, dual-Kawase blur, custom
open/close/resize animations). Populated in milestone M8; see `ROADMAP.md` §3H and
`COMPOSITOR.md` §10.

Shaders are compiled at build time and embedded, so changes here are picked up by
`cargo build`.
