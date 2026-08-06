# Дизайн-система

Полное описание — `docs/design-system.md`, обоснование слоёв — `docs/decisions/0004-design-system-layers.md`. Читать до добавления любого стиля.

## Слои
- `src/tokens.css` — **единственный** файл, где допустимы сырые цвета и размеры. Слои: палитра → семантика → покомпонентные токены.
- `src/index.css` — база и общие классы: `.page`, `.container`, `.app-header`, `.btn` (+модификаторы), `.input`, `.alert`, `.badge`, `.progress`, `.media-grid`, `.poster`, `.hero`, `.surface-glass`, `.marker-resume`, `.auth-*`.
- Всё остальное — в `<style>` конкретного компонента (Svelte скоупит автоматически).

## Правила
- Никаких utility-классов в разметке.
- Никаких `px` — корневой `font-size` масштабирует весь UI (22px на телевизоре).
- Переиспользуемая разметка — это компонент, а не скопированный класс. Перед новой разметкой проверить `src/components/`: `AppHeader`, `Modal`, `Section`/`Row`, `ScreenMessage`, `DetailBackdrop`, `MediaTile`, `Poster`/`PosterPlaceholder`, `SeasonSelector`, `TitleEditor`.
- Правила проверяет stylelint (`npm run lint:css`, входит в `npm run build`). Исключение — только построчный `/* stylelint-disable-next-line … -- причина */`, **никогда** правка конфига.
