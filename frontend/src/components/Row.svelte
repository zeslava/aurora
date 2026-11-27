<script lang="ts">
  import type { Snippet } from 'svelte';

  interface Props {
    /** Ссылка на контейнер: нужна вызывающему для наведения фокуса на первую плитку */
    element?: HTMLElement | null;
    children: Snippet;
  }

  let { element = $bindable(null), children }: Props = $props();

  let carousel: HTMLDivElement;
  let canPrevious = $state(false);
  let canNext = $state(false);
  let activeIndex = 0;
  let movingWithArrow = false;

  const items = () => (element ? Array.from(element.children) as HTMLElement[] : []);
  const behavior = (): ScrollBehavior =>
    window.matchMedia('(prefers-reduced-motion: reduce)').matches ? 'auto' : 'smooth';

  const measure = () => {
    if (!element) return null;
    const style = getComputedStyle(element);
    const padding = parseFloat(style.paddingLeft);
    const gap = parseFloat(style.columnGap);
    const step = (items()[0]?.offsetWidth ?? 0) + gap;
    return { padding, gap, step, max: element.scrollWidth - element.clientWidth };
  };

  const updateEdges = () => {
    const geometry = measure();
    if (!element || !geometry) return;
    // Округление CSS-пикселей на ТВ не должно оставлять стрелку на самом краю.
    canPrevious = element.scrollLeft > 1;
    canNext = element.scrollLeft < geometry.max - 1;
  };

  const reveal = (item: HTMLElement) => {
    const geometry = measure();
    if (!element || !geometry || !geometry.step) return;
    const visible = Math.max(1, Math.floor(
      (element.clientWidth - geometry.padding * 2 + geometry.gap) / geometry.step
    ));
    // Оставляем карточку справа от фокуса: ряд движется раньше, чем фокус упрётся в край.
    const left = item.offsetLeft - geometry.padding - Math.max(0, visible - 2) * geometry.step;
    element.scrollTo({ left: Math.max(0, Math.min(left, geometry.max)), behavior: behavior() });
  };

  const handleFocus = (event: FocusEvent) => {
    const index = items().indexOf(event.target as HTMLElement);
    if (index < 0) return;
    activeIndex = index;
    // Наведение Magic Remote не должно уводить карточку из-под курсора.
    if (movingWithArrow || document.documentElement.dataset.inputMode === 'pointer') return;
    carousel.scrollIntoView({ block: 'nearest', inline: 'nearest', behavior: behavior() });
    reveal(items()[index]);
  };

  const handleKey = (event: KeyboardEvent) => {
    const cards = items();
    if (!cards.length) return;
    const direction = event.key === 'ArrowRight' || event.keyCode === 39 ? 1
      : event.key === 'ArrowLeft' || event.keyCode === 37 ? -1 : 0;
    if (direction) {
      // Стрелки ленты не участвуют в геометрическом поиске соседей: только сами карточки.
      event.preventDefault();
      event.stopPropagation();
      const next = Math.max(0, Math.min(activeIndex + direction, cards.length - 1));
      // На крайней карточке focusin не придёт повторно: после курсора раскрываем её явно.
      if (document.activeElement === cards[next]) reveal(cards[next]);
      cards[next].focus({ preventScroll: true });
    } else if ((event.key === 'ArrowUp' || event.key === 'ArrowDown' ||
      event.keyCode === 38 || event.keyCode === 40) &&
      (event.target as HTMLElement).hasAttribute('data-carousel-control')) {
      // После курсора возвращаем D-pad к карточкам, чтобы вверх/вниз меняли строку.
      cards[Math.min(activeIndex, cards.length - 1)].focus({ preventScroll: true });
    }
  };

  const move = (direction: number) => {
    const geometry = measure();
    const cards = items();
    if (!element || !geometry || !cards.length || !geometry.step) return;
    const left = Math.max(0, Math.min(element.scrollLeft + direction * geometry.step, geometry.max));
    const index = Math.min(cards.length - 1, Math.round(left / geometry.step));
    movingWithArrow = true;
    cards[index].focus({ preventScroll: true });
    movingWithArrow = false;
    element.scrollTo({ left, behavior: behavior() });
  };

  $effect(() => {
    const row = element;
    if (!row) return;
    const resize = new ResizeObserver(updateEdges);
    const mutation = new MutationObserver(updateEdges);
    resize.observe(row);
    mutation.observe(row, { childList: true });
    row.addEventListener('scroll', updateEdges, { passive: true });
    carousel.addEventListener('focusin', handleFocus);
    carousel.addEventListener('keydown', handleKey);
    updateEdges();
    return () => {
      resize.disconnect();
      mutation.disconnect();
      row.removeEventListener('scroll', updateEdges);
      carousel.removeEventListener('focusin', handleFocus);
      carousel.removeEventListener('keydown', handleKey);
    };
  });
</script>

<div class="carousel" class:carousel--before={canPrevious} class:carousel--after={canNext} bind:this={carousel}>
  <div class="row" data-carousel bind:this={element}>
    {@render children()}
  </div>
  <!-- Курсорные стрелки дублируют D-pad и не добавляют остановок между карточками. -->
  <button
    class="carousel__arrow carousel__arrow--previous"
    class:carousel__arrow--hidden={!canPrevious}
    type="button"
    tabindex="-1"
    data-carousel-control
    aria-label="Прокрутить назад"
    onclick={() => move(-1)}
  ><span aria-hidden="true">‹</span></button>
  <button
    class="carousel__arrow carousel__arrow--next"
    class:carousel__arrow--hidden={!canNext}
    type="button"
    tabindex="-1"
    data-carousel-control
    aria-label="Прокрутить вперёд"
    onclick={() => move(1)}
  ><span aria-hidden="true">›</span></button>
</div>

<style>
  .carousel {
    position: relative;
    min-width: 0;
  }

  /* Горизонтальная лента: отступы компенсируют увеличение плитки при фокусе */
  .row {
    position: relative;
    display: flex;
    gap: var(--space-4);
    overflow-x: auto;
    padding: var(--space-4) var(--space-3);
    margin: calc(var(--space-2) * -1) calc(var(--space-3) * -1);
    scrollbar-width: none;
  }

  .row::-webkit-scrollbar {
    display: none;
  }

  .carousel::before,
  .carousel::after {
    content: '';
    position: absolute;
    top: 0;
    bottom: 0;
    width: var(--space-8);
    z-index: var(--z-raised);
    pointer-events: none;
    opacity: 0;
    transition: opacity var(--transition);
  }

  .carousel::before {
    left: calc(var(--space-3) * -1);
    background: linear-gradient(to right, var(--bg), transparent);
  }

  .carousel::after {
    right: calc(var(--space-3) * -1);
    background: linear-gradient(to left, var(--bg), transparent);
  }

  .carousel--before::before,
  .carousel--after::after {
    opacity: 1;
  }

  .carousel__arrow {
    position: absolute;
    top: 50%;
    transform: translateY(-50%);
    z-index: var(--z-overlay);
    display: flex;
    align-items: center;
    justify-content: center;
    width: var(--space-12);
    height: var(--space-12);
    padding: 0;
    border: 1px solid var(--border);
    border-radius: var(--radius-full);
    background-color: var(--bg);
    color: var(--text);
    font-size: var(--text-3xl);
    line-height: 1;
    box-shadow: var(--shadow-lg);
    cursor: pointer;
    transition: background-color var(--transition);
  }

  .carousel__arrow:hover {
    background-color: var(--surface-2);
  }

  .carousel__arrow--previous {
    left: calc(var(--space-3) * -1);
  }

  .carousel__arrow--next {
    right: calc(var(--space-3) * -1);
  }

  .carousel__arrow--hidden {
    visibility: hidden;
    pointer-events: none;
  }
</style>
