<script lang="ts">
  import MediaTile from './MediaTile.svelte';
  import { navigate } from '../lib/router.svelte';
  import type { ContinueWatchingItem } from '../types';

  interface Props {
    item: ContinueWatchingItem;
  }

  let { item }: Props = $props();

  const handleClick = () => {
    if (item.type === 'tvseries' && item.series_id) {
      navigate(`/watch/${item.series_id}/${item.season_number}/${item.episode_number}`);
    } else {
      navigate(`/watch/${item.id}`);
    }
  };

  let percent = $derived(
    item.duration ? Math.min(100, Math.round((item.position / item.duration) * 100)) : 0
  );

  let subtitle = $derived(
    item.type === 'tvseries'
      ? `S${item.season_number}·E${item.episode_number}${item.episode_title ? ` — ${item.episode_title}` : ''}`
      : null
  );
</script>

<MediaTile
  title={item.title}
  poster={item.backdrop}
  {subtitle}
  icon="▶"
  actionLabel="Продолжить"
  progressPercent={percent}
  variant="row"
  onclick={handleClick}
/>
