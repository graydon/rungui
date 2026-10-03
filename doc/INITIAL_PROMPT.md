project goal: portable desktop GUI library in rust

target platforms: win32, macos, linux

approach: build shallow abstractions over most-popular native toolkits on each platform. link directly to platform libraries.

development approach: build in devcontainer (linux). use mingw-w64 to simulate win32 development and clang with gnustep runtime + headers to emulate mac development. build scaffolding to allow same library to build in hosted-on-target-OS mode and hosted-on-linux mode. support cross-compiling mode when possible.

a11y: essential, use accesskit.

i18n: essential, ensure unicode throughout.

rust support: use any of cacao/objc, gtk/gtk4, and winapi/winsafe as needed to either support the platforms you're targeting or as input to study/copy from.

implementation style: keep it simple, don't worry about maximum performance or avoiding runtime book-keeping if that simplifies structure. try to avoid sources of runtime faults (panics, deadlocks). mutable state is ok, old toolkits probably assume some of it. obey platform quirks and requirements about threading and isolation. delegate as much as possible to platform toolkits. if necessary (eg. if pointer-ownership and lifetime issues are hard to make work) it's fine to maintain a separate graph of rust objects identified by (weak, symbolic) handles placed in native toolkit and vice-versa (native toolkit entities identified by weak-or-symbolic handles stored in rust objects). try to support the majority of common widgets but do not over-stretch abstraction when there is no commonality: either skip uncommon features or make some features optional special cases in a way that can fall back to common cases on platforms that don't support. don't try to hide which platform you're on, allow users to extract native handles when they need them if they want to write specialized code per-platform. above all else: keep code small and straightforward. do not use too many traits or too many macros in the user-facing abstractions (it's ok to use them in implementation if it helps you avoid boilerplate). your goal is to produce something on the level of abstraction and complexity as FLTK, libui, IUP, Tk, or similar. But implemented 100% in rust (besides declarations of external platform libraries and linking against them).

note: you're in a devcontainer, you've got sudo, you can install anything you need to help accomplish this goal. but try to stick with linux-hosted tools, maybe go as far as Wine for win32; don't build any foreign-OS virtual machines or anything.