<script lang="ts">
  import Poster from './Poster.svelte';

  interface Props {
    /** Относительный путь кадра с бэкенда; null — плейсхолдер по названию */
    backdrop?: string | null;
    title: string;
    icon?: string;
  }

  let { backdrop = null, title, icon = '🎬' }: Props = $props();
</script>

<!-- Кадр во весь экран: фон страницы деталей, затемнённый градиентом.
     Остаётся на месте при прокрутке контента. -->
<div class="detail-bg" aria-hidden="true">
  <Poster poster={backdrop} {title} {icon} />
</div>
