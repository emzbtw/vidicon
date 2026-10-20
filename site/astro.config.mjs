// @ts-check
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';

// A GitHub Pages project site: served under /vidicon/.
export default defineConfig({
	site: 'https://emzbtw.github.io',
	base: '/vidicon',
	integrations: [
		starlight({
			title: 'Vidicon',
			social: [{ icon: 'github', label: 'GitHub', href: 'https://github.com/emzbtw/vidicon' }],
			sidebar: [{ label: 'Docs', items: [{ autogenerate: { directory: 'docs' } }] }],
		}),
	],
});
