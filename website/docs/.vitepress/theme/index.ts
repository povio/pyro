import DefaultTheme from 'vitepress/theme';
import { useRoute } from 'vitepress';
import type { Theme } from 'vitepress';
import { h } from 'vue';
import PyroPanda from './PyroPanda.vue';
import WptStatus from './WptStatus.vue';
import './custom.css';

export default {
  extends: DefaultTheme,
  enhanceApp({ app }) {
    app.component('WptStatus', WptStatus);
  },
  Layout: () => {
    const route = useRoute();
    return h(DefaultTheme.Layout, null, {
      'home-hero-image': () => h(PyroPanda),
      // /status/wpt/<dir>.html are copies of /status/wpt.html, unknown to the router (WptStatus.vue).
      'not-found': () =>
        /\/status\/wpt\/.+\.html$/.test(route.path)
          ? h('div', { class: 'VPDoc wpt-status-page wpt-status-standalone' }, [h('div', { class: 'vp-doc' }, [h(WptStatus)])])
          : h(DefaultTheme.NotFound),
    });
  },
} satisfies Theme;
