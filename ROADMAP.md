# autorust — роадмап учебного проекта

Инструмент, который читает диагностики `cargo clippy`/`cargo check` в JSON и применяет предложенные компилятором правки к исходникам.

**Формат работы:** ты пишешь код сам, я объясняю концепцию перед этапом и ревьюю результат. Каждый этап заканчивается проверяемым состоянием — если этап не закрыт, не идём дальше.

**Правило масштаба:** проект разогревочный. Если этап начинает разрастаться — режем область, а не растягиваем сроки.

---

## Целевая архитектура (к чему придём)

Держим в голове с самого начала, но реализуем по частям:

```
src/
  main.rs        — точка входа, разбор аргументов, код возврата
  cli.rs         — структура аргументов (clap)
  runner.rs      — запуск cargo clippy подпроцессом, захват stdout
  diagnostics.rs — модель данных JSON-диагностик + парсинг (serde)
  fixes.rs       — извлечение правок из диагностик, фильтры
  apply.rs       — применение правок к тексту файла (offsets, порядок)
  conflicts.rs   — разрешение пересечений
  report.rs      — вывод: diff, dry-run, статистика
```

Ключевая идея разделения: `apply.rs` работает **со строками в памяти**, ничего не знает про файловую систему и про cargo. Это ровно то решение, которое принял `rustfix`, и оно радикально упрощает тестирование. Придерживаемся его.

Поток данных:

```
cargo clippy --message-format=json
      │  (stdout, по одному JSON-объекту на строку)
      ▼
Vec<CargoMessage>  ──фильтр по типу──▶  Vec<Diagnostic>
      ▼
Vec<Fix>  (файл + список Replacement, атомарная единица)
      ▼
фильтры (applicability / lint / путь / макросы) + дедупликация
      ▼
группировка по файлу ──▶ разрешение конфликтов ──▶ применение
      ▼
diff в stdout  ИЛИ  запись файлов
```

---

## Этап 0. Минимальный разогрев по Rust

Не «выучить язык», а закрыть ровно тот набор, без которого первые этапы будут мучением.

**Что изучаем:**
- `String` vs `&str`, почему их два и почему строку нельзя индексировать числом
- `Option<T>`, `Result<T, E>`, оператор `?`
- `Vec<T>`, базовые итераторы (`iter`, `map`, `filter`, `collect`)
- `struct` + `#[derive(Debug, Clone)]`, `enum` + `match`
- модули: `mod`, `pub`, как `src/foo.rs` подключается в `main.rs`
- `cargo new`, `cargo add`, `cargo run`, `cargo clippy`, структура `Cargo.toml`

**Что НЕ нужно сейчас:** лайфтаймы с явной аннотацией, трейты со дженериками, `async`, `unsafe`, `Rc`/`RefCell`, макросы.

**Теория:**
- [The Rust Book](https://doc.rust-lang.org/book/) — главы 3, 4, 6, 7, 8, 9, 13. Остальное потом.
- [Строки и UTF-8](https://doc.rust-lang.org/book/ch08-02-strings.html) — читать внимательно, это ядро проекта
- [Обработка ошибок](https://doc.rust-lang.org/book/ch09-00-error-handling.html)
- [Rust by Example](https://doc.rust-lang.org/rust-by-example/) — как справочник «как это пишется»
- [Rustlings](https://github.com/rust-lang/rustlings) — если хочется руками; достаточно разделов до `iterators`
- [The Cargo Book](https://doc.rust-lang.org/cargo/) — не читать целиком, только оглавление, чтобы знать где искать

**Как проверить:** написать 30-строчную программу, которая читает файл в `String`, находит в нём подстроку и печатает её байтовое смещение. Если это получилось без подглядывания — этап закрыт.

---

## Этап 1. Подопытный крейт и разведка JSON (кода не пишем)

Самый важный этап. Здесь ты своими глазами видишь данные, с которыми будешь работать все остальные этапы. Пропустить его — значит проектировать модель данных наугад.

**Что делаем:**
1. `cargo new --lib testcrate` рядом с проектом
2. Пишем туда 5–6 функций с заведомыми warning'ами разных видов:
   - неиспользуемая переменная (правка = добавить `_`)
   - неиспользуемый `use` (правка = удалить, `suggested_replacement: ""`)
   - `clippy::redundant_clone`
   - `clippy::needless_return`
   - `clippy::len_zero` (`x.len() == 0` → `x.is_empty()`)
   - что-нибудь с `MaybeIncorrect` для контраста
   - что-нибудь с кириллицей/эмодзи в строковом литерале **до** места правки — понадобится на этапе 5
3. `cargo clippy --message-format=json > diags.json` внутри testcrate
4. Читаем `diags.json` руками. Полностью. С документацией рядом.

**Что проектируем (на бумаге, не в коде):**
- Какие поля `span` нам реально нужны: `file_name`, `byte_start`, `byte_end`, `suggested_replacement`, `suggestion_applicability`, `expansion`
- Где лежит настоящий совет: не в `spans` верхнего уровня диагностики, а в `children[].spans[]` с `level: "help"`. Это первое, на чём спотыкаются все, кто пишет такой инструмент.
- Что делать с диагностикой, у которой несколько `spans` с `suggested_replacement` — это **одна атомарная правка** из нескольких кусков (применять все или ни один)
- Как отличить обёртку cargo (`{"reason": "compiler-message", ...}`) от самой диагностики rustc

**Теория:**
- [JSON Output — The rustc book](https://doc.rust-lang.org/rustc/json.html) — **основной документ проекта**. Здесь построчно описана схема диагностики, все поля `span`, и все значения `suggestion_applicability` с объяснениями.
- [JSON messages — The Cargo Book](https://doc.rust-lang.org/cargo/reference/external-tools.html#json-messages) — как cargo оборачивает сообщения rustc, что за `reason`, `target`, `build-finished`
- [Errors and lints — rustc dev guide](https://rustc-dev-guide.rust-lang.org/diagnostics.html) — раздел про suggestions: как авторы линтов выбирают уровень applicability и почему `MachineApplicable` заслуживает доверия
- [Список линтов clippy](https://rust-lang.github.io/rust-clippy/master/) — фильтр «Applicability: MachineApplicable» покажет, с чем ты вообще сможешь работать
- [Уровни линтов](https://doc.rust-lang.org/rustc/lints/levels.html)

**Грабли, которые вылезут прямо здесь:**
- **Кэш.** Второй `cargo clippy` вернёт пустоту — результат закэширован. Лечится `touch src/*.rs` или `cargo clean`. Забудешь — будешь час отлаживать «парсер, который ничего не находит».
- **Дубли.** Один файл компилируется для нескольких таргетов (lib, test, bench) → одна и та же правка приедет несколько раз. Дедупликация нужна будет обязательно.
- **Пути.** `file_name` относителен рабочей директории cargo, а не твоей.

**Готово когда:** ты можешь на словах объяснить, из каких полей какой правки складывается конкретное изменение в конкретном файле.

---

## Этап 2. Каркас: запуск clippy и захват вывода

Первый код. Сознательно маленький: ничего не парсим, только получаем строки.

**Что проектируем:**
- Аргументы MVP: путь к целевому крейту (`--manifest-path`), и всё
- Что делать, если `cargo` не найден / вернул ненулевой код / крейт не компилируется вообще

**Что пишем:** `runner.rs` — функция, возвращающая `Vec<String>` (строки stdout) или ошибку. `main.rs` печатает количество строк.

**Что изучаем:**
- `std::process::Command`: `args`, `current_dir`, `stdout(Stdio::piped())`, `output()` vs `spawn()`
- почему JSON-сообщения cargo идут в **stdout**, а человекочитаемый прогресс — в stderr
- `anyhow` для ошибок в приложении (не библиотеке) — избавляет от ручных `enum Error` на первых этапах

**Теория:**
- [`std::process::Command`](https://doc.rust-lang.org/std/process/struct.Command.html)
- [`std::process::Output`](https://doc.rust-lang.org/std/process/struct.Output.html)
- [`anyhow`](https://docs.rs/anyhow/) — раздел про `Context`
- [`--message-format`](https://doc.rust-lang.org/cargo/commands/cargo-clippy.html) в справке cargo

**Решение к обсуждению на этом этапе:** `output()` (собрать всё, потом обработать) или `spawn()` + построчное чтение? Для MVP хватит `output()`. Обсудим, когда дойдём.

**Как проверить:** запустить на testcrate, сравнить число строк с `wc -l diags.json`.

---

## Этап 3. Модель данных и парсинг

**Что проектируем:**
- Структуры: `CargoMessage`, `Diagnostic`, `DiagnosticSpan`, `DiagnosticCode`, `Applicability`
- Что делаем с неизвестными полями (игнорируем — `serde` так по умолчанию) и с неизвестными значениями `applicability` (rustc-книга прямо предупреждает, что значения могут добавляться → нужен вариант `Other`)
- Что делаем со строкой, которая не распарсилась: падаем или пропускаем? (Пропускаем со счётчиком — cargo может подмешать не-JSON.)

**Что пишем:** `diagnostics.rs` — структуры с `#[derive(Deserialize)]` + функция `parse_lines(&[String]) -> Vec<Diagnostic>`.

**Что изучаем:**
- `serde` derive, `#[serde(rename_all = "snake_case")]`, `#[serde(other)]` для enum
- `Option<T>` в полях = «может быть null»
- `serde_json::from_str`, тип ошибки

**Теория:**
- [serde — Derive](https://serde.rs/derive.html)
- [serde — Атрибуты полей](https://serde.rs/field-attrs.html) и [контейнера](https://serde.rs/container-attrs.html)
- [serde — Enum representations](https://serde.rs/enum-representations.html) — пригодится для `reason`-обёртки cargo
- [`serde_json`](https://docs.rs/serde_json/)
- [`cargo_metadata`](https://docs.rs/cargo_metadata/) — **не подключаем**, но заглядываем в исходники: там уже описаны те же структуры, полезно сравнить со своими

**Как проверить:** unit-тест, где JSON-строка вбита прямо в тест (возьми настоящую из `diags.json` этапа 1) → проверяем распарсенные поля. Это первый тест в проекте.

**Готово когда:** на `diags.json` из testcrate парсер выдаёт ожидаемое число диагностик, ни одна строка не потерялась молча.

---

## Этап 4. От диагностик к правкам

Слой перевода: диагностика — это сообщение для человека, правка — это машинная операция. Смешивать их в одной структуре не надо.

**Что проектируем:**
```rust
struct Replacement { byte_start: usize, byte_end: usize, text: String }
struct Fix {
    file: PathBuf,
    lint: Option<String>,
    applicability: Applicability,
    message: String,
    replacements: Vec<Replacement>,  // атомарно: все или ничего
}
```
- Обход `children` рекурсивно: где искать `suggested_replacement`
- Фильтры: только `MachineApplicable`; только пути внутри целевого крейта; `expansion == None` (правки внутри раскрытия макроса пропускаем)
- Дедупликация одинаковых `Fix`

**Что пишем:** `fixes.rs` — `fn collect_fixes(&[Diagnostic], &Config) -> Vec<Fix>`.

**Что изучаем:**
- `PathBuf`/`Path`, канонизация путей, `starts_with` для проверки «внутри крейта»
- `HashSet` / `sort` + `dedup` для дедупликации, `#[derive(PartialEq, Eq, Hash)]`
- рекурсивный обход дерева на Rust без боли

**Теория:**
- [`std::path`](https://doc.rust-lang.org/std/path/index.html)
- [`Vec::dedup_by_key`](https://doc.rust-lang.org/std/vec/struct.Vec.html#method.dedup_by_key), [`HashSet`](https://doc.rust-lang.org/std/collections/struct.HashSet.html)
- [rust-lang/rust#88514](https://github.com/rust-lang/rust/issues/88514) — реальный баг: `MachineApplicable`-правка указывает в файл внутри `~/.cargo/registry`. Прочитай, чтобы понять, зачем фильтр по путям обязателен, а не «на всякий случай».
- [`rustfix::Filter`](https://docs.rs/rustfix/latest/rustfix/enum.Filter.html) — посмотри, как ту же задачу оформили в готовом крейте (сравниваем после того, как написал своё)

**Как проверить:** прогон на testcrate → печать таблицы «файл : строка : линт : что заменяем на что». Глазами сверить с выводом обычного `cargo clippy`.

---

## Этап 5. Применение одной правки

Здесь начинается настоящая предметная сложность. Файловую систему пока не трогаем.

**Что проектируем:** чистая функция `fn apply_one(source: &str, r: &Replacement) -> Result<String>`.

**Что изучаем и где грабли:**
- `byte_start`/`byte_end` — **байтовые** смещения от начала файла, а не позиция в символах и не позиция в строке
- `String::replace_range` **паникует**, если границы не на границе UTF-8-символа. Отсюда нужна валидация: `is_char_boundary`
- проверка на устаревшие данные: если файл изменили после запуска clippy, оффсеты указывают в пустоту. Нужен sanity-check — сравнить текст в диапазоне с тем, что clippy показал в `span.text`
- `suggested_replacement: ""` — это удаление, отдельного кода не требует, но проверь, что не сломается

**Теория:**
- [`String::replace_range`](https://doc.rust-lang.org/std/string/struct.String.html#method.replace_range)
- [`str::is_char_boundary`](https://doc.rust-lang.org/std/primitive.str.html#method.is_char_boundary)
- [`str::get`](https://doc.rust-lang.org/std/primitive.str.html#method.get) — безопасное срезание по диапазону, возвращает `Option`
- [Строки и UTF-8 в Rust Book](https://doc.rust-lang.org/book/ch08-02-strings.html) — перечитать сейчас, когда есть контекст
- [`std::fs::read_to_string`](https://doc.rust-lang.org/std/fs/fn.read_to_string.html) / [`write`](https://doc.rust-lang.org/std/fs/fn.write.html)

**Как проверить:** unit-тесты на строках в памяти. Обязательный кейс — правка **после** строкового литерала с кириллицей: если у тебя где-то проскочила арифметика по символам вместо байтов, этот тест её поймает, а тест на ASCII — нет.

**Готово когда:** MVP из пункта 3 исходного плана — прогнали clippy на testcrate, применили одну правку к настоящему файлу, `cargo build` в testcrate проходит.

---

## Этап 6. Пакетное применение

**Что проектируем:** `fn apply_all(source: &str, fixes: &[Fix]) -> Result<String>`.

Центральный алгоритмический момент проекта:
- сортировка правок по `byte_start` **по убыванию** и применение с конца файла к началу — тогда оффсеты ещё не применённых правок остаются валидными
- альтернатива: идти с начала и вести накопленный сдвиг (`delta`). Разберём оба варианта и почему первый предпочтительнее (меньше состояния, меньше шансов ошибиться)
- атомарность `Fix` с несколькими `Replacement`
- что если два `Replacement` внутри одного `Fix` пересекаются (бывает, и это баг линта — надо отбросить весь `Fix`)

**Что изучаем:**
- `sort_by_key`, `sort_by`, стабильность сортировки и почему она здесь важна
- как аккуратно вернуть ошибку из середины итерации (`?` внутри цикла, `collect::<Result<Vec<_>>>`)

**Теория:**
- [`slice::sort_by_key`](https://doc.rust-lang.org/std/primitive.slice.html#method.sort_by_key), [`sort_unstable_by`](https://doc.rust-lang.org/std/primitive.slice.html#method.sort_unstable_by)
- [`Iterator`](https://doc.rust-lang.org/book/ch13-02-iterators.html)
- [`collect` в `Result`](https://doc.rust-lang.org/rust-by-example/error/iter_result.html)
- [`rustfix::CodeFix`](https://docs.rs/rustfix/latest/rustfix/struct.CodeFix.html) — читаем **после** того, как написал своё, и сравниваем подходы

**Как проверить:** testcrate с 5+ warning'ами в одном файле, применяем все, `git diff`, потом `cargo build` + повторный `cargo clippy` (должно стать чище).

---

## Этап 7. Безопасность: dry-run, diff, git

Пункт 4 исходного плана. Раньше не делаем, потому что до этапа 6 нечего было показывать.

**Что проектируем:**
- `--dry-run`: ничего не пишем, печатаем unified diff
- проверка чистого git-состояния, флаг обхода (`--allow-dirty`, как у `cargo fix`)
- формат вывода: что поменялось, в каком файле, из-за какого линта
- запись файлов: сразу или через temp + rename (атомарность)

**Что изучаем:**
- генерация diff: крейт [`similar`](https://docs.rs/similar/) (`TextDiff::from_lines` + `unified_diff`)
- запуск `git status --porcelain` подпроцессом — проще, чем тянуть `git2`
- раскраска вывода: [`owo-colors`](https://docs.rs/owo-colors/) или [`anstream`](https://docs.rs/anstream/); отключение цвета при не-TTY

**Теория:**
- [`similar`](https://docs.rs/similar/) — раздел про `udiff`
- [`cargo fix`](https://doc.rust-lang.org/cargo/commands/cargo-fix.html) — как устроены его флаги безопасности (`--allow-dirty`, `--allow-staged`, `--broken-code`). Хорошая референсная модель: не изобретай свой UX, начни с копии этого.
- [`tempfile`](https://docs.rs/tempfile/) — если пойдём в атомарную запись

**Как проверить:** `--dry-run` на грязном дереве → отказ; diff в выводе совпадает с реальным `git diff` после запуска без `--dry-run`.

---

## Этап 8. Конфликты и пересечения

Пункт 5 исходного плана.

**Что проектируем — таксономия случаев:**
1. правки не пересекаются → применяем все
2. правки в одинаковых диапазонах и с одинаковым текстом → дубль, применяем одну
3. вложенные диапазоны (одна внутри другой) → применяем внешнюю? внутреннюю? ни одну?
4. частично пересекающиеся → нельзя применить обе, нужна политика
5. правка нулевой длины (вставка) на границе другой правки → отдельный неочевидный случай

**Стратегия:** консервативная — при конфликте применяем первую по некоторому детерминированному порядку, остальные конфликтующие откладываем и сообщаем о них. Повторный запуск инструмента их подберёт.

**Что изучаем:** проектирование алгоритма на интервалах, детерминированность (одинаковый вход → одинаковый выход, иначе не протестируешь).

**Теория:**
- [`rustfix::apply_suggestions`](https://docs.rs/rustfix/latest/rustfix/fn.apply_suggestions.html) — в описании прямо сказано, что она разрешает часть конфликтов автоматически; смотрим, какие именно
- [Исходники rustfix](https://docs.rs/crate/rustfix/latest/source/src/lib.rs) — короткий и читаемый файл, отличное чтение на этом этапе
- [`cargo fix` и итерации](https://doc.rust-lang.org/cargo/commands/cargo-fix.html) — почему cargo fix запускает компиляцию несколько раз

**Как проверить:** искусственный тест-кейс с двумя линтами, чьи spans накладываются (например, `needless_return` внутри выражения, которое трогает другой линт). Собрать такой кейс — само по себе полезное упражнение.

---

## Этап 9. Тесты

Формально можно было раньше, но именно здесь набирается критическая масса.

**Что проектируем:**
- unit-тесты на `apply.rs` и `conflicts.rs` — чистые функции, тестируются строками, без файлов
- интеграционные тесты: временный крейт → запуск бинарника → проверка результата
- фикстуры: пары «до / после» в отдельной папке

**Что изучаем:**
- `#[cfg(test)] mod tests`, `assert_eq!`, `#[should_panic]`
- `tests/` директория и как она отличается от unit-тестов
- `assert_cmd` + `predicates` — запуск своего CLI из теста
- `tempfile` — временные крейты
- `insta` — снапшот-тесты (идеально для diff-вывода)

**Теория:**
- [Тестирование — Rust Book](https://doc.rust-lang.org/book/ch11-00-testing.html)
- [`assert_cmd`](https://docs.rs/assert_cmd/), [`predicates`](https://docs.rs/predicates/)
- [`tempfile`](https://docs.rs/tempfile/)
- [`insta`](https://docs.rs/insta/) — раздел про review workflow
- [Как тестируется rustfix](https://docs.rs/crate/rustfix/latest/source/) — папка `tests`, схема «`.rs` + `.fixed.rs`». Хорошая идея, украдём.

---

## Этап 10. CLI-полировка и CI-режим

Пункт 6 исходного плана.

**Что проектируем:**
- `--lint <name>` / `--exclude-lint`, фильтр по категориям clippy
- `--applicability <level>` — осознанно разрешить `MaybeIncorrect` под свою ответственность
- коды возврата: 0 — нечего править / всё применено; 1 — есть непримененные правки (для CI); 2 — ошибка инструмента
- `--check` для CI: не менять файлы, вернуть ненулевой код, если правки есть

**Что изучаем:**
- `clap` derive API: `#[derive(Parser)]`, подкоманды, `ValueEnum` для уровней
- `ExitCode` vs `std::process::exit` и почему первое лучше
- `--` для прокидывания аргументов в cargo

**Теория:**
- [`clap` derive](https://docs.rs/clap/latest/clap/_derive/index.html) и [cookbook](https://docs.rs/clap/latest/clap/_cookbook/index.html)
- [Пример cargo-подкоманды в clap](https://docs.rs/clap/latest/clap/_cookbook/cargo_example_derive/index.html) — если захочешь, чтобы работало как `cargo autorust`
- [`std::process::ExitCode`](https://doc.rust-lang.org/std/process/struct.ExitCode.html)
- [Custom subcommands — Cargo Book](https://doc.rust-lang.org/cargo/reference/external-tools.html#custom-subcommands)
- [Rust API Guidelines](https://rust-lang.github.io/api-guidelines/) — по верхам, для причёсывания публичных имён

---

## Этап 11. Сравнение с rustfix и cargo fix

Пункт 7 исходного плана — и заодно точка, где проект даёт максимум учебной отдачи.

**Что делаем:**
1. Берём реальный открытый репозиторий среднего размера
2. Прогоняем на копии: свой инструмент vs `cargo clippy --fix`
3. Сравниваем diff'ы. Разбираем каждое расхождение: у кого баг?
4. Читаем `rustfix` целиком и пишем список «что он делает, а я нет»

**Теория:**
- [`rustfix`](https://docs.rs/rustfix/) — теперь читаем всё, включая описание крейта (обрати внимание на предупреждение, что API нестабилен и крейт не для внешнего использования — это само по себе полезный урок про экосистему)
- [`cargo::ops::fix`](https://github.com/rust-lang/cargo/blob/master/src/cargo/ops/fix/mod.rs) — как cargo оборачивает rustfix: как он подсовывает себя вместо rustc через `RUSTC_WORKSPACE_WRAPPER`, как делает несколько проходов, как откатывает сломанный код. Самая интересная часть всей темы.

**Готово когда:** ты можешь на одной странице описать, чем твой инструмент отличается от `cargo fix` и какие из отличий — сознательные решения, а какие — недоделки.

---

## Этап 12. Опциональные расширения (только если захочется)

Строго по одному, и только после того, как этапы 1–11 закрыты:

- **Итерации до неподвижной точки:** применили правки → перезапустили clippy → повторили, пока правки не кончатся или не превышен лимит проходов. Так работает `cargo fix`.
- **Откат сломанного кода:** после применения проверяем `cargo check`; если сломалось — возвращаем файлы как было.
- **Workspace:** несколько пакетов, `cargo metadata` для определения границ.
- **Стриминг:** обрабатывать JSON построчно по мере поступления, а не собирать весь вывод.
- **Собственный лог-вывод:** `tracing` вместо `println!`.

---

## Сводная таблица

| Этап | Компонент | Главная новая идея | Ключевая ссылка |
|---|---|---|---|
| 0 | — | базовый Rust | [Rust Book](https://doc.rust-lang.org/book/) |
| 1 | testcrate | схема JSON-диагностик | [rustc JSON](https://doc.rust-lang.org/rustc/json.html) |
| 2 | `runner.rs` | подпроцесс, stdout | [`Command`](https://doc.rust-lang.org/std/process/struct.Command.html) |
| 3 | `diagnostics.rs` | десериализация | [serde derive](https://serde.rs/derive.html) |
| 4 | `fixes.rs` | своя модель ≠ модель JSON | [`rustfix::Filter`](https://docs.rs/rustfix/latest/rustfix/enum.Filter.html) |
| 5 | `apply.rs` | байты и границы UTF-8 | [`replace_range`](https://doc.rust-lang.org/std/string/struct.String.html#method.replace_range) |
| 6 | `apply.rs` | порядок применения, сдвиг оффсетов | [`rustfix::CodeFix`](https://docs.rs/rustfix/latest/rustfix/struct.CodeFix.html) |
| 7 | `report.rs` | dry-run, diff, git | [`cargo fix`](https://doc.rust-lang.org/cargo/commands/cargo-fix.html) |
| 8 | `conflicts.rs` | интервалы и политики | [`apply_suggestions`](https://docs.rs/rustfix/latest/rustfix/fn.apply_suggestions.html) |
| 9 | `tests/` | чистые функции тестируемы | [Book ch. 11](https://doc.rust-lang.org/book/ch11-00-testing.html) |
| 10 | `cli.rs` | UX и коды возврата | [`clap` derive](https://docs.rs/clap/latest/clap/_derive/index.html) |
| 11 | — | сравнение с эталоном | [`cargo::ops::fix`](https://github.com/rust-lang/cargo/blob/master/src/cargo/ops/fix/mod.rs) |

---

## Список грабель — держать под рукой

1. **Кэш cargo** — повторный прогон не выдаёт warning'ов. `touch` исходники.
2. **Совет лежит в `children`**, а не в `spans` верхнего уровня.
3. **Дубли из-за нескольких таргетов** (lib/test/bench компилируют один файл).
4. **Spans в чужие крейты** (`~/.cargo/registry`) — [известный баг](https://github.com/rust-lang/rust/issues/88514). Фильтр по путям обязателен.
5. **`expansion != null`** — правка внутри раскрытия макроса, применять нельзя.
6. **Байты, не символы.** `byte_start` — это байты от начала файла.
7. **Границы UTF-8** — `replace_range` паникует, `str::get` возвращает `None`.
8. **Порядок применения** — с конца файла к началу.
9. **Атомарность** — один `Fix` может состоять из нескольких `Replacement`.
10. **`suggested_replacement: ""`** = удаление, не «замена на пустоту по ошибке».
11. **Устаревшие оффсеты** — файл могли изменить между clippy и применением. Проверяй текст в диапазоне.
12. **Enum'ы расширяемы** — rustc-книга прямо предупреждает: новые значения `applicability` могут появиться. Нужен `Other`.

---

## С чего начинаем

Этап 0 можно сокращать по вкусу, но **этап 1 пропускать нельзя**. Предлагаю следующим сообщением договориться о содержимом testcrate — какие именно линты туда закладываем, чтобы получить интересный набор диагностик — а потом ты пришлёшь мне `diags.json`, и мы разберём его вместе, поле за полем.