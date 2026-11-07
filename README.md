# KERN

Нативная IDE на Rust: редактор кода, поиск, терминал, запуск проектов, Git, GitHub, чат с Codex в правой панели, официальный Claude Code во вкладке терминала. Без Electron, WebView и HTML-интерфейса. Платформы: macOS на Apple Silicon (arm64) и Windows 11 x64. Версия 0.1.1.

## Download

**[Download for Windows (x64)](https://github.com/meytyy/kern/releases/download/v0.1.1/KERN-windows-x64-setup.exe)** · **[Download for macOS (Apple Silicon)](https://github.com/meytyy/kern/releases/download/v0.1.1/KERN-macos-arm64.zip)**

Version 0.1.1. Both builds are unsigned: Windows SmartScreen and macOS Gatekeeper ask before the first start. On macOS, open KERN.app from the context menu (Open) or allow it in System Settings → Privacy & Security.

Редактор, поиск, терминал и локальный Git работают без аккаунта и независимо от агентов.

### Updates

A few seconds after start KERN asks the GitHub Releases API of `meytyy/kern` for the newest stable release (drafts, pre-releases and `-rc`/`-beta` tags are skipped) and compares it with its own version by SemVer, so `v0.10.0` is newer than `v0.9.0`. If it is newer and has a file for this platform, a small notice offers **Download Update**, which opens that file's direct link in the browser, or **Later**. Nothing is downloaded or installed by KERN, and the notice appears at most once per launch. **Help → Check for Updates…** checks again and also says when KERN is up to date, when the release has no build for this platform, or why the check failed (no network, GitHub rate limit). The request goes through the system `curl`.

Release files must keep these names, which `scripts/bundle.sh` and `scripts/build-windows.ps1` produce:

| Platform | File |
|---|---|
| macOS Apple Silicon | `KERN-macos-arm64.zip` |
| Windows x64 | `KERN-windows-x64-setup.exe` (preferred), `KERN-windows-x64.zip` |

## Codex

The panel on the right is Codex. Press ⌘↩ (Ctrl+Enter on Windows) to send; Enter starts a new line. It uses the official App Server and your existing Codex sign-in through ChatGPT: history and resume, model and reasoning effort from `model/list`, attachments, streaming, Stop, permission requests and Review Diff.

The Usage popover shows what the connection reports and nothing else: tokens from `thread/tokenUsage/updated`, subscription limits from `account/rateLimits/read` and `account/rateLimits/updated`.

Attachments: `+ → Attach Files`, drag from Finder, Explorer or KERN's Explorer, and ⌘V/Ctrl+V for clipboard images. Images are sent as `localImage` inputs, never as a path in the text; TIFF and BMP clipboard images are converted to PNG first. Text files go inside the message. Pasted images are kept in `attachments/` in KERN's config folder. Files like `.env` need a separate confirmation.

## Claude Code

Claude Code is the official `claude` CLI in its own tab of the bottom panel, on a real PTY (Unix PTY on macOS, ConPTY on Windows) in the project folder. **Claude Code: Open** in the command palette and **Open** on its card in Extensions do the same thing: they focus the project's live session, or start one. There is never a second process for a project. Hiding the bottom panel keeps the process and its output; **End Claude Code Session** (the trash icon on that tab, or the palette) ends it. Sign-in, subscription and history stay with Claude Code; KERN does not read its terminal output or its credentials.

KERN no longer has a graphical Claude chat. What it left behind is not deleted: the Agent SDK in `claude-sdk/` of KERN's config folder (about 370 MB, remove it by hand if you like), an API key in the macOS Keychain item `dev.kern.ide` if you saved one, and `claude-chat` entries in `agent-sessions.json`. The official CLI's sign-in is untouched.

The bottom panel is per project: hidden until first used, then 20% of the working area, and it remembers its height and visibility. Hiding it (×, ⌘J, ⌃\`) keeps PTY sessions running; only Kill Terminal or End Claude Code Session ends one.

The **Terminal** menu (in the macOS menu bar, and in the title bar on Windows) has New Terminal, Show / Hide Terminal, Kill Terminal and Open Claude Code. They are the same actions as the panel buttons, ⌃\` and the command palette. Kill Terminal ends only the selected terminal session.

## Explorer

New File and New Folder are in the Explorer header, in the right-click menu, in the File menu and in the palette; all of them use the same prompt and the same creation code. The header, the File menu and the palette create inside the selected folder, in the selected file's folder, or in the project root. The right-click menu goes by where you click: a folder row creates inside that folder, a file row in its folder, and the empty space below the last row in the project root, whatever is selected. Ctrl+Click opens the same menu on a Mac. Below New File… and New Folder… the menu has Reveal in Finder (Show in File Explorer on Windows), Open in Integrated Terminal (a shell in that folder), Copy Path, Copy Relative Path and, on rows, Rename…. It opens at the pointer, stays inside the window, and closes with Esc or a click outside. Enter confirms and Esc cancels. Names may contain spaces and Cyrillic, start with a dot (`.gitignore`) or name nested folders (`src/new.ts`). Windows rules apply on both platforms: no `< > : " | ? * \`, no trailing dot or space, no `CON`, `NUL`, `COM1`…; a name that exists is reported in the prompt and never overwritten; nothing can be created outside the project, including through a symbolic link. A new file opens in a focused tab; a new folder is shown and selected in the tree. Git status refreshes afterwards.

## File icons

`src/ui/file_icons.rs` is the one registry for the Explorer and editor tabs: exact names and patterns first, then extensions. The SVG set is generated by `scripts/make_file_icons.py`.

## Быстрый старт

Нужны Rust и Xcode Command Line Tools. Полный Xcode не требуется: шейдеры Metal компилируются при запуске.

```sh
xcode-select -p                 # проверка Command Line Tools; если ошибка: xcode-select --install
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh    # если нет Rust

cargo run                       # dev-версия
cargo run -- /путь/к/проекту    # dev-версия сразу с проектом
cargo build --release           # release-бинарник: target/release/kern
scripts/bundle.sh               # упаковка: dist/KERN.app и dist/KERN-macos-arm64.zip
open dist/KERN.app              # запуск как из Finder
```

Зависимости зафиксированы в `Cargo.lock`, `scripts/bundle.sh` собирает с `--locked`.

## Статус

**Локальная сборка: работает.** `dist/KERN.app` (arm64, ad-hoc подпись) открывается через LaunchServices и из терминала, в Dock видна своя иконка.

**Публичное распространение: не готово.** Нет подписи Developer ID и notarization, без них Gatekeeper заблокирует скачанное приложение на другом Mac. Вход ChatGPT для публичной версии тоже не решён, см. раздел про Codex.

## Внешние инструменты

Все необязательны для обычного редактирования. Если инструмента нет, KERN показывает статус и команду установки.

| Для чего | Инструмент | Установка |
|---|---|---|
| Git | `git` | `xcode-select --install` |
| Vite, Next.js | `node`, менеджер пакетов | `brew install node` |
| GitHub | `gh` | `brew install gh` |
| Codex | `codex` | `brew reinstall --cask codex` |
| Claude Code | `claude` | `curl -fsSL https://claude.ai/install.sh \| bash` |
| TS/JS, TypeScript 7+ | `tsc` из проекта | `npm install -D typescript` или `npm install -g typescript` |
| TS/JS, TypeScript 5–6 | `typescript-language-server` | `npm install -g typescript@5 typescript-language-server` |
| Python | `pyright-langserver` | `npm install -g pyright` |
| Rust | `rust-analyzer` | `rustup component add rust-analyzer` |

### Пути к инструментам

Приложение из Finder получает минимальный `PATH`. Поэтому при старте KERN в фоне спрашивает окружение у вашей login-оболочки (`$SHELL -l -i -c env`, таймаут 8 секунд) и добавляет типовые папки: `/opt/homebrew/bin`, `/usr/local/bin`, `~/.local/bin`, `~/.cargo/bin`.

Если инструмент всё равно не находится, укажите путь вручную: **Settings → Set Tool Path** (имя и абсолютный путь). Битые симлинки определяются отдельно и показываются как `broken`.

Настройки и сессии лежат в `~/Library/Application Support/KERN/settings.json`.

## Сочетания клавиш

Все сочетания и их подписи берутся из одной таблицы, `src/ui/keys.rs`: из неё строятся привязки клавиш, пункты меню, подсказки, палитра команд и подсказка в поле Codex. На Windows интерфейс пишет Ctrl, Alt, Shift и Enter, а не ⌘ и ⌥.

| Действие | macOS | Windows |
|---|---|---|
| Сохранить | ⌘S | Ctrl+S |
| Быстро открыть файл | ⌘P | Ctrl+P |
| Палитра команд | ⇧⌘P | Ctrl+Shift+P |
| Поиск по проекту | ⇧⌘F | Ctrl+Shift+F |
| Настройки | ⌘, | Ctrl+, |
| Отправить в Codex | ⌘↩ | Ctrl+Enter |
| Перенос строк | ⌥Z | Alt+Z |
| Закрыть вкладку, новый файл | ⌘W, ⌘N | Ctrl+W, Ctrl+N |
| Перейти к определению | F12 | F12 |
| Run, Stop | ⌘R, ⌘. | Ctrl+R, Ctrl+. |
| Боковая и нижняя панели | ⌘B, ⌘J | Ctrl+B, Ctrl+J |
| Терминал | ⌃\` | Ctrl+\` |
| Панель Codex | ⌥⌘C | Ctrl+Shift+A |
| Extensions | ⇧⌘X | Ctrl+Shift+X |

Сохранить, закрыть вкладку, новый файл и перенос строк работают только в редакторе. В терминале Tab уходит оболочке, а Ctrl+C без выделения прерывает команду; на Windows Ctrl+P/B/J/O/Q/R/./, при фокусе в терминале тоже достаются оболочке. В Settings Tab переходит между полями. На Windows ни одно сочетание не использует Ctrl+Alt: это AltGr.

## Языки

Реестр: `src/lang/mod.rs`. Возможности разделены: определение файла, подсветка, сервер и запуск. «Сервер поддерживается KERN» и «сервер есть на этом Mac» — разные вещи: фактическое состояние показывает **Settings → Languages**, инструмент там реально запускается.

Тест `every_tree_sitter_language_really_highlights` парсит пример на каждом языке и требует разных цветов. Он нашёл, что в `gpui-component` у Swift и CMake пустой запрос подсветки, а у TSX и C++ нет базовых правил. KERN регистрирует их заново (`src/ui/grammars.rs`).

C# в `gpui-component` не было вовсе, поэтому `.cs` открывался как обычный текст. Теперь грамматика `tree-sitter-c-sharp` регистрируется в `src/ui/grammars.rs`. Запрос подсветки лежит в `assets/queries/csharp-highlights.scm` с другим порядком правил: подсветчик KERN оставляет за узлом первое совпадение, а в исходном запросе первым стоит общее `(identifier) @variable`, которое перекрасило бы типы и методы. Тест `csharp_program_uses_many_colors` проверяет `tests/fixtures/Program.cs` настоящим подсветчиком и темой.

| Язык | Определение | Подсветка | Языковой сервер | Запуск |
|---|---|---|---|---|
| JavaScript, JSX | `.js .mjs .cjs .jsx` | Tree-sitter | TypeScript проекта или глобальный | скрипты `package.json` |
| TypeScript, TSX | `.ts .mts .cts .tsx` | Tree-sitter | TS 7+: `tsc --lsp`; TS 5–6: `typescript-language-server` | скрипты `package.json` |
| Python | `.py .pyi` | Tree-sitter | `pyright-langserver`, `basedpyright-langserver` | через интерпретатор проекта |
| Rust | `.rs` | Tree-sitter | `rust-analyzer` | Cargo |
| Swift | `.swift` | Tree-sitter | `sourcekit-lsp` | `swift` при `Package.swift` |
| C, C++ | `.c .h .cpp .cc .cxx .hpp .hh .mm` | Tree-sitter | `clangd` | CMake |
| C# | `.cs .csx` | Tree-sitter (`tree-sitter-c-sharp`), сразу при открытии, без .NET SDK | `csharp-ls`, если установлен (`dotnet tool install --global csharp-ls`) | нет |
| CMake | `CMakeLists.txt .cmake` | Tree-sitter | `cmake-language-server` | `cmake` |
| Shell | `.sh .bash .zsh .zshrc` | Tree-sitter | `bash-language-server` | текущий скрипт |
| HTML | `.html .htm` | Tree-sitter | `vscode-html-language-server` | нет |
| CSS | `.css .scss` | Tree-sitter | `vscode-css-language-server` | нет |
| JSON | `.json .jsonc` | Tree-sitter | `vscode-json-language-server` | нет |
| YAML | `.yaml .yml` | Tree-sitter | `yaml-language-server` | нет |
| TOML | `.toml Cargo.lock` | Tree-sitter | нет | нет |
| Markdown | `.md .markdown .mdx` | Tree-sitter | нет | нет |
| Inno Setup | `.iss .isl` | лексер KERN: секции, ключи, комментарии, строки, препроцессор; `[Code]` базово | нет | нет |
| Dockerfile | `Dockerfile Dockerfile.* .dockerfile` | лексер KERN | `docker-langserver` | нет |
| Env | `.env .env.*` | лексер KERN | нет | нет |
| Ignore | `.gitignore .dockerignore` и др. | лексер KERN | нет | нет |

Смена языка: клик по языку в строке состояния или **Editor: Change Language Mode**. Выбор действует на один файл проекта или запоминается для имени или расширения. Свой сервер добавляется в **Settings → Languages**.

**TypeScript.** Выбор идёт по версии TypeScript в проекте. TS 7+ работает через встроенный `tsc --lsp --stdio`, в нём нет `tsserver.js`. TS 5–6 работает через `typescript-language-server` с `tsserver` проекта.

**Next.js.** Подсказки TS-плагина Next работают на TS 5.x: проверено значение `force-static` у `export const dynamic`. На TS 7 плагины из tsconfig этой проверкой не покрыты, считайте их недоступными.

**Доверие к папке.** Языковые серверы исполняют код проекта: build-скрипты, плагины tsserver, интерпретатор Python. Поэтому при первом открытии папки KERN спрашивает разрешение. До него LSP выключен, остальное работает. Команды проекта запускаются только по кнопке Run.

## Безопасность данных

- Файл сохраняется атомарно, с сохранением BOM, окончаний строк (включая смешанные) и прав доступа.
- Если на диске файл изменился, чистый буфер обновляется. Для буфера с правками появляется панель выбора, сама ни одна версия не перезаписывается.
- Бинарные файлы и файлы не в UTF-8 не открываются, чтобы сохранение их не испортило.
- Токены не хранятся. Вход GitHub держит `gh` в системной связке ключей, вход Codex и Claude Code держат их официальные клиенты.

## Интерфейс

Слева Files, Search, Source Control, Extensions; шестерёнка открывает Settings. В центре вкладки файлов, diff, настроек, расширений и медиа, над кодом breadcrumbs. Снизу Terminal, Run Output и Claude Code, если расширение включено. Справа Agents.

Кнопки окна системные. Тема: Graphite (нейтральный тёмно-серый: редактор #191B1E, панели #222428, текст #E7E9EC, фокус и выделение светло-серые), Cream (светлая) или System (следует за светлым или тёмным режимом ОС). Настройка `navy` из прежних сборок читается как Graphite.

## Редактор

Word Wrap выключен по умолчанию: ⌥Z, команда, кнопка `Wrap` в строке состояния или Settings. Шрифт, размер, номера строк, таб, направляющие и активная строка настраиваются в **⌘, → Editor** и сразу применяются к открытым файлам.

## Git Graph

Внизу Source Control: цветные дорожки, развилки, слияния, метки веток и тегов, HEAD. Current Branch / All Branches, Fetch, Refresh, Copy Hash. История грузится по 200 коммитов вне UI-потока. Клик показывает сообщение, автора, дату, хеш и файлы; у merge-коммита видно, с каким родителем идёт сравнение, и родителя можно выбрать. Просмотр только читает репозиторий.

## Extensions

Собственный встроенный каталог из двух расширений: Codex (правая панель) и Claude Code (вкладка нижней панели) (⇧⌘X). Сторонний код, VSIX и Marketplace не поддерживаются. Состояния Installed, Enabled, CLI Ready и Signed In определяются по факту. Install и Uninstall меняют только состояние в KERN: внешний CLI, вход и история не трогаются. KERN не обходит предупреждения macOS и не запускает чужие копии.

## История агентов

В локальном индексе `agent-sessions.json` только агент, проект, официальный id, заголовок и время. Переписку хранят официальные клиенты.

- **Codex:** `thread/list`, `thread/resume`, `thread/name/set`.
- **Claude Code:** история у самого CLI (`/resume` внутри сессии). Идентификаторы из текста терминала не извлекаются.

Если KERN запущен из сессии Claude Code, он убирает её маркеры окружения: иначе дочерний `claude` не сохраняет переписку.

## Фото и видео

| Формат | Внутри KERN |
|---|---|
| PNG, JPEG, WebP, GIF | да, GIF анимируется в активном окне |
| SVG | да, внешние ссылки вырезаются и не загружаются |
| MP4, MOV с кодеками macOS (H.264 проверен) | да: AVPlayer, кадры через `CVPixelBuffer` в раскладке GPUI |
| WebM, AVI, MKV (проверенные образцы) | нет, понятное сообщение и Open With Default App |

Изображения: Fit, 100%, приближение, панорамирование, размеры, формат, шахматный фон. Видео открывается на паузе; при уходе со вкладки ставится на паузу, при закрытии плеер освобождается. Обрезанный JPEG показывается с предупреждением.

## GitHub

Clone from GitHub использует `gh`: существующий вход переиспользуется, иначе `gh auth login --web` открывается в окне терминала. Список репозиториев берётся через `gh repo list`.

Для github.com Git получает `gh` как дополнительный credential helper только на время своих команд, глобальный конфиг Git не меняется. Git и clone по URL от GitHub не зависят.

Open Pull Request после push только открывает страницу создания PR в браузере.

## Codex

KERN запускает `codex app-server` и общается с ним по JSONL на stdin/stdout. Методы сверены со схемой `codex app-server generate-json-schema` версии 0.167. Запускается только по действию пользователя.

KERN не задаёт `approvalPolicy` и `sandbox`, поэтому действуют ваши настройки Codex. Запросы разрешений показываются в панели. Хранилище токенов не читается, содержимое сообщений не логируется. `.env`, ключи и похожие файлы автоматически не прикрепляются.

**Review Diff.** Перед задачей KERN снимает снимок рабочей папки в отдельный Git-каталог в данных приложения, ваш репозиторий и его индекс не затрагиваются. Файл получает одну из меток:

| Метка | Значение | Откат |
|---|---|---|
| `agent` | Правка пришла как `fileChange` от Codex, пользователь файл не трогал | Да, если файл не менялся после задачи |
| `conflict` | Пересеклась с несохранённой или сохранённой пользователем правкой | Нет, только diff |
| `diff only` | Изменён во время задачи, но нельзя надёжно приписать агенту (например, shell-команда) | Нет, только diff |

Файлы из `.gitignore`, `node_modules`, `target`, `.venv`, `dist`, `build`, `.next` в снимок не входят, и их изменения в Review не видны.

### Личная интеграция и распространение

**Личная сборка.** Вход выполняет ваш установленный Codex CLI через официальный ChatGPT-вход. KERN только открывает ссылку в браузере и показывает статус. Это проверено: `account/read` с существующим входом и настоящие задачи.

**Публичное распространение: не готово и не заявляется.** Страница OpenAI [Sign in with ChatGPT](https://developers.openai.com/siwc/token-sharing-open-source) описывает использование плана ChatGPT для open-source и локально размещённых приложений, с согласием пользователя и обязательным `ext_agent_host_id`. Для платных или удалённо размещённых приложений нужна форма заявки. Документация [App Server](https://learn.chatgpt.com/docs/app-server) просит идентифицировать клиент через `clientInfo.name`, а для корпоративного использования обратиться в OpenAI. Перед публикацией условия нужно проверить заново, доступ не проверялся.

## Выбор стека

**GPUI** (через `gpui-kit` 0.9) прошёл проверку: окно, редактируемый текст с кириллицей, скролл, вызов системного диалога (`examples/gpui_probe.rs`). Компоненты: редактор с Tree-sitter и LSP-провайдерами, diff, поля ввода. Терминал написан поверх `alacritty_terminal`.

Что выяснилось при интеграции:
- В `gpui-component` TSX регистрируется без базовых правил JS, поэтому KERN регистрирует грамматику заново (`src/ui/grammars.rs`).
- Номер кадра GIF хранится в состоянии элемента `img`, поэтому ему нужен `id`.
- Колонки в LSP-типах виджета считаются в символах Unicode, а серверы работают в UTF-16. Конвертация вынесена в `src/lsp/position.rs` и `src/ui/lsp_bridge.rs`.

## Структура

```
src/core       тексты, состояние буферов, настройки
src/workspace  дерево файлов, поиск, слежение за диском
src/lang       реестр языков
src/lsp        LSP-клиент, кодировки позиций
src/terminal   PTY-сессии, клавиатура
src/run        конфигурации, процессы, разбор вывода
src/git        Git CLI, разбор статуса, граф коммитов
src/github.rs  подключение через gh
src/agents     протокол и клиент Codex, индекс чатов, снимки, вложения
src/platform   платформенные детали: процессы, окружение, инструменты, видео macOS
src/ui         интерфейс на GPUI
```

GPUI подключён только в `src/ui`. Unix-специфика собрана в `src/platform/unix.rs` и `src/terminal`, это задел для порта на Windows.

## Проверки

```sh
cargo fmt --check
cargo test
cargo clippy --all-targets      # нужен: rustup component add clippy
```

На Windows тесты, которые запускают настоящие Unix-процессы (PTY, группы процессов, тестовые LSP-серверы на `/bin/sh`), не собираются: `#[cfg(unix)]`.

Тесты покрывают то, где ошибка портит данные или ломает протокол: кодирование файлов, конфликты буфера и диска, позиции LSP, разбор Git-статуса, Git с локальным remote, настоящие PTY, остановку групп процессов, сообщения Codex, снимки и откат.

Фича `autotest` добавляет сценарный драйвер, который гоняет настоящее окно и настоящие инструменты. В обычную сборку не входит:

```sh
cargo run --features autotest   # с KERN_AUTOTEST=путь/к/сценарию; шаги в src/ui/autotest.rs
```

## Известные ограничения

- Один проект в одном окне.
- Виджет редактора рассчитан примерно на 50 тысяч строк по документации библиотеки. Файлы больше 32 МБ не открываются, больше 4 МБ открываются без подсветки.
- LSP получает полный текст файла при каждой правке (full sync), на очень больших файлах это заметно.
- На приближение изображения влияют ⌘ или Ctrl с прокруткой; жест щипка не обрабатывается.
- Stop завершает группу процессов (на Windows дерево процессов через `taskkill /T`). Процесс, создавший свою сессию через `setsid`, может пережить Stop.
- Выделение в Run Output построчное: клик и Shift+клик.

## Клавиша Tab

Tab обрабатывается по активной панели:

- **Terminal** — `0x09` уходит в PTY, Shift+Tab — как `ESC [ Z`; дополняет сама оболочка.
- **Редактор** — принимает выбранную подсказку, иначе делает отступ по стилю файла: Makefile и Go всегда табуляцией, остальные файлы по их содержимому, новые по настройке Tab Size. `.editorconfig` не читается.
- **Settings, диалоги, формы, поле чата** — Tab и Shift+Tab переключают фокус. Диалог держит фокус внутри себя.

Если конфигурация zsh не загружает систему дополнений, KERN запускает `compinit` только для своих терминалов. Файлы пользователя не меняются: запуск идёт через папку `zsh-startup` в настройках KERN, а `ZDOTDIR` восстанавливается до первого приглашения.

## Windows 11 x64

Общий код один для обеих платформ; отличается только `src/platform/` и несколько мест с `cfg`.

| | macOS | Windows |
|---|---|---|
| Окно | системные кнопки macOS | свои кнопки Свернуть / Развернуть / Закрыть, перетаскивание, двойной клик и Snap через системный hit-test |
| Терминал | Unix PTY, оболочка из `$SHELL` | ConPTY (`alacritty_terminal`), по умолчанию `pwsh`, затем Windows PowerShell, затем `cmd.exe`; выбор в Settings → Terminal → Shell |
| Копирование в терминале | ⌘C / ⌘V | Ctrl+C копирует выделение, без выделения прерывает команду; Ctrl+Shift+C/V, Ctrl+V |
| Сочетания | ⌘ | Ctrl; панель Codex — Ctrl+Shift+A (Ctrl+Alt на Windows это AltGr). С фокусом в терминале Ctrl+P/B/J/O/Q/R/./, достаются оболочке |
| Поиск инструментов | `PATH` из login shell, Homebrew | `Path` процесса и `PATHEXT` (`.exe`, `.cmd`), плюс `Program Files\nodejs`, `%APPDATA%\npm`, `~\.local\bin`, `~\.cargo\bin`, Git, GitHub CLI |
| Python | `.venv/bin/python`, `python3` | `.venv\Scripts\python.exe`, `python` |
| Stop | SIGTERM, затем SIGKILL группе | `taskkill /T`, затем `/T /F` |
| Видео во вкладке | AVFoundation | пока нет: вкладка объясняет причину и даёт Open With Default App |
| Секреты | нет своих | нет своих |

### Сборка на Windows (ASUS)

Требования: Windows 11 x64; Rust через rustup (stable, target `x86_64-pc-windows-msvc`); Visual Studio 2022 Build Tools с «Разработка классических приложений на C++» (MSVC v143 и Windows 11 SDK: из SDK берутся `fxc.exe` для шейдеров GPUI и `rc.exe` для иконки). Установщик собирается, если есть Inno Setup 6.

```powershell
winget install Rustlang.Rustup
winget install Microsoft.VisualStudio.2022.BuildTools --override "--quiet --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
winget install JRSoftware.InnoSetup
cd C:\путь\к\KERN
powershell -ExecutionPolicy Bypass -File scripts\build-windows.ps1
```

Результат: `dist\windows\KERN\KERN.exe` (рядом ничего не нужно: C-рантайм вшит статически), `dist\windows\KERN-windows-x64.zip` и с Inno Setup `dist\windows\KERN-windows-x64-setup.exe`. Установщик кладёт ярлык в меню Пуск, поддерживает удаление и не трогает настройки и историю в `%APPDATA%\kern`. Сборка не подписана: SmartScreen может спросить при первом запуске.

Иконка Windows (`assets/windows/kern.ico`) собирается из исходного `logo.png`, который не меняется: `swift scripts/make_win_icon.swift logo.png assets/windows/kern.ico`. В ней размеры 16, 24, 32, 48, 64, 128 (32-битный BMP с альфой) и 256 (PNG); снаружи скруглённого квадрата всё прозрачно, поля одинаковые. `python3 scripts/check_ico.py assets/windows/kern.ico` проверяет размеры, прозрачность углов и поля. Иконка macOS собирается отдельно (`scripts/make_icon.swift`) на непрозрачной подложке, потому что macOS сама накладывает свою маску. Workflow **Windows icon check** ставит KERN поверх прошлого релиза и начисто и снимает ярлык на светлом и тёмном рабочем столе, в Проводнике и на панели задач.

С Mac для пробного запуска: `scripts/cross-windows.sh` (нужны `cargo-xwin` и `brew install llvm`). Эта сборка компилирует шейдеры GPUI при старте через `d3dcompiler_47.dll` из Windows (профиль `xwin`), потому что `fxc.exe` есть только на Windows.

Перенос на ASUS: папка проекта без `target`, `dist`, `node_modules`, `.env` и других секретов.
