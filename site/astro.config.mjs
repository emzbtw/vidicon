// @ts-check
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';
import starlightLinksValidator from 'starlight-links-validator';
import readmePages from './readme-pages.mjs';

// A GitHub Pages project site: served under /vidicon/.
export default defineConfig({
	site: 'https://emzbtw.github.io',
	base: '/vidicon',
	integrations: [
		readmePages(),
		starlight({
			title: 'Vidicon',
			social: [{ icon: 'github', label: 'GitHub', href: 'https://github.com/emzbtw/vidicon' }],
			sidebar: [{ label: 'Docs', items: [{ autogenerate: { directory: 'docs' } }] }],
			// A broken internal link fails the build.
			plugins: [starlightLinksValidator()],
		}),
	],
});
