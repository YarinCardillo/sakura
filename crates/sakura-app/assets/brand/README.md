# Brand files

These assets are generated outside this repository and are used byte for byte. Do not edit them here.

| File | What | SHA-256 |
|-|-|-|
| `cells.json` | The mark: the lit cells of its 32 x 32 grid (`on`, as `[column, row]`), its two colours, and the lockup's geometry | `53a05e91690fcbd5ab68376b202354c2f120cac6c26d57f7097dc33d43e25009` |
| `lockup-on-dark.svg` | Mark and word for a dark ground, 123 x 32 cells | `81bedc708b70b349e055c53bee978253af365daa7e25aa9480155fa23684b3fe` |
| `lockup-on-light.svg` | Mark and word for a light ground | `f9f78c735684087b7b4d9441eb3eef16074902a01d066991ac4262f2c57c5b49` |

The app icon comes from the same generator: `assets/icons/Sakura.icns` at the root of the repository
(`4c51050ffef64517fe238a269bed2b48047a7bc687268f64ed0a5d51d114f4df`), which the macOS bundle uses as it is.

`src/ui/brand` reads these files and holds the rules they are shown by.

`assets/icons/sakura-1024.png` is the same brand app icon rendered at 1024 x 1024 pixels, used by the Linux package.
