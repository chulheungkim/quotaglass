# QuotaGlass Brand Assets

The approved app mark is **Meter Pane**: one liquid-glass squircle containing
three embedded usage meters.

## Files

- `quotaglass-meter-pane-concept.png` is the approved presentation render on a
  neutral background.
- `quotaglass-meter-pane-master.png` is the production artwork with a
  transparent background.
- `../../icon-source.png` is the generated macOS/Tauri source export.

## App icon export

Run:

```bash
pnpm icon
```

This regenerates `icon-source.png` and every platform icon under
`src-tauri/icons/`.

The existing macOS optical-sizing adjustment is intentional and must remain
unchanged: the artwork is fitted to an 850×850 box centered on a transparent
1024×1024 canvas, leaving 8.5% padding on every side.

Do not add another square behind the glass squircle, remove the transparent
padding, or add a diagonal tail.
