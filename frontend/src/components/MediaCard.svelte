<script lang="ts">
  import MediaTile from './MediaTile.svelte';
  import { navigate } from '../lib/router.svelte';
  import type { MediaItem, PlaybackState } from '../types';

  interface Props {
    media: MediaItem;
    progress?: PlaybackState | null;
  }

  let { media, progress = null }: Props = $props();

  const handleClick = () => {
    if (media.type === 'tvseries') {
      navigate(`/series/${media.id}`);
    } else {
      navigate(`/media/${media.id}`);
    }
  };

  let progressPercent = $derived(
    progress ? Math.min(100, Math.round((progress.position / (media.duration || 1)) * 100)) : 0
  );
</script>

<MediaTile
  title={media.title}
  poster={media.backdrop}
  subtitle={media.year ? String(media.year) : null}
  badge={media.resolution}
  icon={media.type === 'tvseries' ? '📺' : '🎬'}
  actionLabel={progressPercent > 5 ? 'Продолжить' : 'Смотреть'}
  {progressPercent}
  onclick={handleClick}
>
  {#snippet footer()}
    {#if progress?.completed}
      <span class="badge badge--success watched">Просмотрено</span>
    {/if}
  {/snippet}
</MediaTile>

<style>
  .watched {
    margin-top: var(--space-2);
  }
</style>
