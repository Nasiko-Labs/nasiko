# Fonts

The two `.woff2` files in this folder are copied, unchanged, from the Fontsource packages the app
depends on (`oss/app/package.json`), so that the Vite asset graph and `fonts.css` can self-host them. Both static entry documents preload the same files:

| File                                     | Package                               | Version | Family                                                | Weight axis |
| ---------------------------------------- | ------------------------------------- | ------- | ----------------------------------------------------- | ----------- |
| `hanken-grotesk-latin-wght-normal.woff2` | `@fontsource-variable/hanken-grotesk` | 5.3.0   | Hanken Grotesk (variable, latin subset, normal style) | 100–900     |
| `jetbrains-mono-latin-wght-normal.woff2` | `@fontsource-variable/jetbrains-mono` | 5.3.0   | JetBrains Mono (variable, latin subset, normal style) | 100–800     |

Both fonts are licensed under the SIL Open Font License 1.1 (`LICENSE-OFL.txt`, the packages' own
`LICENSE` files: the copyright lines of both projects, then the OFL text, which is identical in each).
Upstream: Hanken Grotesk, Copyright 2021 The Hanken Grotesk Project Authors
(https://github.com/marcologous/hanken-grotesk); JetBrains Mono, Copyright 2020 The JetBrains Mono
Project Authors (https://github.com/JetBrains/JetBrainsMono). Packaged by Fontsource
(https://fontsource.org) from Google Fonts.

Latin subset only, as in the OpenRuntime UI Lab (`common/src/index.css` at 1b99bdda): other scripts
fall back to the system font. To update, bump the packages and copy the files again.
