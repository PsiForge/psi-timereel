# Test fixtures

Screenshots used by the size check in `crates/timereel-image/tests/fixture_sizes.rs`.

They contain no personal data. Each one is a 1440x900 to 1920x1080 PNG of an
open-source page (rustdoc pages and rustdoc source view of the `webp`,
`fast_image_resize` and `zune-jpeg` crates, MIT/Apache-2.0 licensed), rendered
by headless Google Chrome from locally built `cargo doc` output:

```
cargo doc -p webp -p fast_image_resize -p zune-jpeg --no-deps
google-chrome --headless=new --window-size=1920,1080 --hide-scrollbars \
  --screenshot=out.png file://$PWD/target/doc/fast_image_resize/struct.Resizer.html
```

They are text-heavy browser windows, not full desktops with a wallpaper. Replace
or extend them with real desktop captures if a more demanding size check is
wanted.
