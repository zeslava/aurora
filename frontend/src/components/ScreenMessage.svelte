<script lang="ts">
  interface Props {
    /** Текст состояния: «Загрузка…», «Медиа не найдено» */
    text: string;
    /** true — экран плеера: подложка чёрная, а не цвет фона приложения */
    cinema?: boolean;
  }

  let { text, cinema = false }: Props = $props();
</script>

<div class="screen-message" class:screen-message--dark={cinema}>{text}</div>
