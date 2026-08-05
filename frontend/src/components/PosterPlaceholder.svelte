<script lang="ts">
  import { hueFor, initialsFor } from '../lib/posterArt';

  interface Props {
    title: string;
    icon?: string;
  }

  let { title, icon = '🎬' }: Props = $props();

  let initials = $derived(initialsFor(title));
  let hue = $derived(hueFor(title));
</script>

<div class="ph" style:--ph-hue={hue}>
  <span class="ph__icon">{icon}</span>
  <span class="ph__initials">{initials}</span>
  <span class="ph__title">{title}</span>
</div>

<style>
  /* Плейсхолдер живёт на цветной подложке, а не на --surface: текст здесь
     белый с прозрачностью, а не токен --text-* */
  .ph {
    /* stylelint-disable function-disallowed-list -- цвет подложки считается
       из хеша названия (hueFor), а текст поверх неё — белый с прозрачностью:
       ни то ни другое не выражается токеном */
    --ph-fg: rgb(255 255 255 / 0.85);
    --ph-fg-dim: rgb(255 255 255 / 0.55);

    container-type: size;
    position: absolute;
    inset: 0;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: var(--space-2);
    padding: var(--space-3);
    background-image: linear-gradient(
      135deg,
      hsl(var(--ph-hue) 45% 28%),
      hsl(calc(var(--ph-hue) + 40) 50% 16%)
    );
    /* stylelint-enable function-disallowed-list */
  }

  .ph__icon {
    position: absolute;
    top: var(--space-2);
    left: var(--space-2);
    font-size: var(--text-base);
    opacity: 0.35;
  }

  .ph__initials {
    /* cqh — чтобы буквы не вылезали на широких 16/9 карточках */
    font-size: min(28cqw, 34cqh);
    font-weight: var(--weight-bold);
    line-height: 1;
    letter-spacing: 0.05em;
    color: var(--ph-fg);
  }

  .ph__title {
    max-width: 100%;
    font-size: max(0.7rem, min(7cqw, 9cqh));
    text-align: center;
    color: var(--ph-fg-dim);
    overflow: hidden;
    display: -webkit-box;
    -webkit-box-orient: vertical;
    -webkit-line-clamp: 2;
    line-clamp: 2;
  }
</style>
