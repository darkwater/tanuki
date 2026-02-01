tanuki
======

Code-first smart home stuff, still in experimental phase. Proper readme will
come later.

The `examples` folder contains the actual way I use this. The idea is that
instead of having a GUI to configure everything, you write code, and the
library aims to make that as easy and convenient as possible, basically
something in the direction of:

```rust
// concept/idea, not actually how it works now
light_switch.on_button_press(|ev| match ev.button {
    "on" => north_light.turn_on(),
    "off" => north_light.turn_off(),
    _ => {}
});
```
