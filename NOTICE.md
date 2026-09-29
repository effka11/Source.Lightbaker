# NOTICE — Source Lightbaker

**Проект:** Source Lightbaker

**Copyright (c) 2026 Egor Ozerskikh (Егор Озерских).**

Этот файл — карта лицензий репозитория. Он не заменяет тексты лицензий
и не перелицензирует чужие компоненты. Для своего кода обязывающий текст —
[`LICENSE`](LICENSE) (GNU Affero General Public License, версия 3).

## 1. Свой код

Под **GNU Affero General Public License, version 3 only (AGPL-3.0-only)**
лицензированы:

- `crates/`;
- `Cargo.toml`, `Cargo.lock`;
- `documents/`, `context.md`, `readme.md`;
- этот `NOTICE.md` и [`LICENSE`](LICENSE).

SPDX: `AGPL-3.0-only`.

Поле `license` в `Cargo.toml` относится только к этому коду.

Лицензия дана **только на версию 3**. Опция «or any later version» из
AGPLv3 §14 **не** предоставляется. Абзац «How to Apply These Terms» в конце
`LICENSE` — шаблон Free Software Foundation, а не условие этого проекта.
Право перевести код на более новую версию лицензии остаётся у
правообладателя.

## 2. Дополнительные условия по AGPLv3 §7

1. **Сохранение уведомлений — §7(b).** Copyright-уведомления, этот
   `NOTICE.md` и файл `LICENSE` сохраняются во всех копиях и существенных
   частях кода.
2. **Нет лицензии на имя — §7(e).** Лицензия на код не даёт прав на
   название «Source Lightbaker», кроме описательной ссылки на происхождение.

## 3. Выход программы

BSP, который программа записывает рядом с картой, — данные на выходе, а не
копия программы. AGPL на этот файл не переходит.

## 4. Зависимые лицензии

Ниже — компоненты, которые программа берёт со стороны. Их лицензии остаются
их лицензиями. При распространении программы их уведомления сохраняются.

Список крейтов снят с `Cargo.lock` для цели `x86_64-pc-windows-msvc`.
После смены зависимостей этот раздел нужно сверить заново: у каждого крейта
поле `license` в его `Cargo.toml` главнее этой карты.

### 4.1. Бинарники рядом с программой

В git их нет. `crates/solve/build.rs` линкует их из `third_party/embree`
(или из `EMBREE_DIR`) и копирует DLL рядом со сборкой. Архив —
`embree-4.4.1.x64.windows.zip`, не SYCL-сборка.

| Компонент | Файлы | Лицензия |
| --- | --- | --- |
| Intel Embree 4.4.1 | `embree4.dll` | Apache-2.0. Copyright Intel Corporation. Текст в распакованном SDK: `third_party/embree/share/doc/Embree_superbuild/LICENSE.txt`. Сопроводительный список чужих программ Intel — `third-party-programs.txt` в том же каталоге. |
| Intel oneAPI Threading Building Blocks | `tbb12.dll`, `tbbmalloc.dll` | Apache-2.0. Copyright Intel Corporation. https://github.com/oneapi-src/oneTBB |

Apache-2.0: http://www.apache.org/licenses/LICENSE-2.0

### 4.2. Прямые крейты

| Крейт | Версия | SPDX |
| --- | --- | --- |
| bytemuck | 1.25.2 | Zlib OR Apache-2.0 OR MIT |
| eframe | 0.32.3 | MIT OR Apache-2.0 |
| glam | 0.29.3 | MIT OR Apache-2.0 |
| rayon | 1.12.0 | MIT OR Apache-2.0 |
| wgpu | 25.0.2 | MIT OR Apache-2.0 |
| zip | 2.4.2 | MIT |

### 4.3. Шрифты окна

`eframe` собран с `default_fonts`, поэтому в программу входит
`epaint_default_fonts` 0.32.3. SPDX этого крейта:
`(MIT OR Apache-2.0) AND OFL-1.1 AND Ubuntu-font-1.0`.
Код крейта — на выбор MIT или Apache-2.0. Шрифты — отдельно, и их
лицензии не заменяются лицензией программы.

| Шрифт | Лицензия |
| --- | --- |
| Hack | MIT, Copyright 2018 Source Foundry Authors. Доля Bitstream Vera — Bitstream Vera License, Copyright 2003 Bitstream, Inc. Имена «Bitstream» и «Vera» зарезервированы. |
| Noto Emoji | SIL Open Font License 1.1 |
| Ubuntu Light | Ubuntu Font Licence 1.0. Шрифт и производные от него остаются под этой лицензией. |
| emoji-icon-font | MIT, Copyright 2014 John Slegers |

Тексты лежат в крейте: `fonts/Hack-Regular.txt`, `fonts/OFL.txt`,
`fonts/UFL.txt`, `fonts/emoji-icon-font-mit-license.txt`.

### 4.4. Остальное дерево крейтов

На Windows в сборку входит 190 сторонних крейтов. Большая часть —
`MIT OR Apache-2.0` и близкие пермиссивные варианты
(`Apache-2.0`, `MIT`, `Zlib`, `0BSD`, `BSD-2-Clause`, `BSD-3-Clause`,
`Unlicense`, `ISC`, `BSL-1.0`, `CC0-1.0`). Исходники их не переписывают.

Отдельного упоминания требуют такие:

- **Unicode-3.0.** `icu_collections`, `icu_locale_core`, `icu_normalizer`,
  `icu_normalizer_data`, `icu_properties`, `icu_properties_data`,
  `icu_provider`, `litemap`, `potential_utf`, `tinystr`, `writeable`,
  `yoke`, `yoke-derive`, `zerofrom`, `zerofrom-derive`, `zerotrie`,
  `zerovec`, `zerovec-derive`.
  `unicode-ident` — `(MIT OR Apache-2.0) AND Unicode-3.0`.
  Copyright © Unicode, Inc. Данные и программы ICU распространяются по
  Unicode License v3: https://www.unicode.org/license.txt
- **Boost Software License 1.0.** `clipboard-win`, `error-code`.
  https://www.boost.org/LICENSE_1_0.txt
- **ISC.** `libloading`.
- **Zlib.** `foldhash`. Вариант Zlib также есть у `bytemuck`,
  `bytemuck_derive`, `miniz_oxide`, `adler2`, `cursor-icon`,
  `raw-window-handle`, `zune-core`, `zune-jpeg`.
- **CC0-1.0.** `hexf-parse`.
- **Apache-2.0 AND MIT** (оба текста сразу, не на выбор). `dpi`.
- **BSD-3-Clause OR Apache-2.0.** `moxcms`, `pxfm`.
- **BSD-2-Clause OR Apache-2.0 OR MIT.** `zerocopy`, `zerocopy-derive`.

## 5. Контакт

Правообладатель: **Egor Ozerskikh** — **e.ozerskikh@gmail.com**
