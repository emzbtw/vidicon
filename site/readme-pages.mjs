// Splits README.md's user-facing sections into Starlight pages under
// src/content/docs/docs/ (git-ignored), so the README stays the only copy
// of that text. Runs at the start of every `astro build` and `astro dev`;
// in dev, an edit to the README restarts the server and regenerates them.
import { mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';
import GithubSlugger from 'github-slugger';

// README `## ` headings that become pages, in sidebar order.
const PAGES = ['Install', 'First run', 'Keys', 'Reporting a bug', 'Disclaimer'];
// README `## ` headings written to src/generated/ (git-ignored) as partials
// for the landing page to import, with no page of their own.
const PARTIALS = ['What it does not do'];

const siteDir = dirname(fileURLToPath(import.meta.url));
const readme = join(siteDir, '..', 'README.md');
const outDir = join(siteDir, 'src', 'content', 'docs', 'docs');
const partialsDir = join(siteDir, 'src', 'generated');

// Split the README into `## ` sections, skipping fenced code blocks.
// The HTML header and intro come before the first `## ` and are dropped.
// A fence may be indented (inside a list item) and use backticks or
// tildes; only a fence of the same character closes it.
export function sections(text) {
	const out = [];
	let fence = null;
	for (const line of text.split('\n')) {
		const marker = line.match(/^\s*(`{3,}|~{3,})/)?.[1][0];
		const fenceLine = Boolean(marker && (!fence || marker === fence));
		if (fenceLine) fence = fence ? null : marker;
		if (!fence && !fenceLine && line.startsWith('## ')) out.push({ title: line.slice(3).trim(), lines: [] });
		else if (out.length) out.at(-1).lines.push({ line, fenced: Boolean(fence) || fenceLine });
	}
	return out;
}

// The bullets of the README's "What it does" section, for the landing
// page's feature cards, from the README's text. A bullet may wrap onto
// indented lines.
export function whatItDoes(text) {
	const section = sections(text).find((s) => s.title === 'What it does');
	if (!section) throw new Error('README.md has no "## What it does" section');
	const bullets = [];
	for (const { line, fenced } of section.lines) {
		if (fenced) continue;
		if (line.startsWith('- ')) bullets.push(line.slice(2).trim());
		else if (bullets.length && line.startsWith('  ') && line.trim()) bullets[bullets.length - 1] += ` ${line.trim()}`;
		else if (bullets.length && !line.trim()) break;
	}
	return bullets;
}

// Where each README anchor lives on the site: a section is its page,
// a subheading is an anchor on its section's page. GitHub slugs the whole
// README at once, so a repeated heading gets `-1` there; Starlight slugs
// each page on its own, so the same heading has no suffix on its page.
export function anchorMap(all, pages) {
	const anchors = new Map();
	const readmeSlugger = new GithubSlugger();
	for (const s of all) {
		const page = pages.find((p) => p.title === s.title);
		const pageSlugger = new GithubSlugger();
		const anchor = readmeSlugger.slug(s.title);
		if (page) anchors.set(anchor, { page: page.slug });
		for (const { line, fenced } of s.lines) {
			const h = !fenced && line.match(/^#{3,} (.*)/);
			if (h) {
				const sub = readmeSlugger.slug(h[1].trim());
				const hash = pageSlugger.slug(h[1].trim());
				if (page) anchors.set(sub, { page: page.slug, hash });
			}
		}
	}
	return anchors;
}

// A section's Markdown with its links rewritten for the site. `page.slug`
// is the page it lands on, and `repoRoot` the repo root relative to the
// generated file.
export function sectionMarkdown(page, anchors, repoRoot) {
	const target = (url, image) => {
		if (url.startsWith('#')) {
			const t = anchors.get(url.slice(1));
			if (!t) return url;
			if (t.page === page.slug && t.hash) return `#${t.hash}`;
			return `/vidicon/docs/${t.page}/${t.hash ? `#${t.hash}` : ''}`;
		}
		if (/^([a-z][a-z+.-]*:|\/|\.\.\/)/i.test(url)) return url;
		// A repo image is served by Astro from its path relative to the
		// generated file, without a copy in site/.
		if (image) return `${repoRoot}/${url}`;
		// Any other repo file has no page on the site: link to it on GitHub.
		return `https://github.com/emzbtw/vidicon/blob/main/${url}`;
	};
	return page.lines
		.map(({ line, fenced }) => {
			if (fenced) return line;
			// Code spans are set aside, so a `](` inside one is not a link.
			const spans = [];
			return (
				line
					// The page title is the h1, so `###` becomes `##`.
					.replace(/^#(#{2,}) /, '$1 ')
					.replace(/`[^`]*`/g, (span) => `\0${spans.push(span) - 1}\0`)
					// `![alt` is kept to tell an image from a link; a title after
					// the target stays outside it.
					.replace(/(!\[[^\]]*)?\]\(([^)\s]+)(\s+"[^"]*")?\)/g, (_, image = '', url, title = '') => `${image}](${target(url, image)}${title})`)
					.replace(/\0(\d+)\0/g, (_, i) => spans[i])
			);
		})
		.join('\n')
		.trim();
}

export function writeReadmePages() {
	const all = sections(readFileSync(readme, 'utf8'));
	const pages = PAGES.map((title) => {
		const section = all.find((s) => s.title === title);
		if (!section) throw new Error(`README.md has no "## ${title}" section`);
		return { ...section, slug: new GithubSlugger().slug(title) };
	});

	const anchors = anchorMap(all, pages);

	// A section's Markdown for a file written to `dir`, with its links
	// rewritten for the site.
	const body = (page, dir) => sectionMarkdown(page, anchors, relative(dir, join(siteDir, '..')));

	rmSync(outDir, { recursive: true, force: true });
	mkdirSync(outDir, { recursive: true });
	pages.forEach((page, i) => {
		const frontmatter = `---\ntitle: ${JSON.stringify(page.title)}\nsidebar:\n  order: ${i + 1}\n---\n\n`;
		writeFileSync(join(outDir, `${page.slug}.md`), `${frontmatter}<!-- Generated from README.md by site/readme-pages.mjs. Edit the README. -->\n\n${body(page, outDir)}\n`);
	});

	rmSync(partialsDir, { recursive: true, force: true });
	mkdirSync(partialsDir, { recursive: true });
	for (const title of PARTIALS) {
		const section = all.find((s) => s.title === title);
		if (!section) throw new Error(`README.md has no "## ${title}" section`);
		const slug = new GithubSlugger().slug(title);
		writeFileSync(join(partialsDir, `${slug}.md`), `<!-- Generated from README.md by site/readme-pages.mjs. Edit the README. -->\n\n${body({ ...section, slug }, partialsDir)}\n`);
	}
}

export default function readmePages() {
	return {
		name: 'readme-pages',
		hooks: {
			'astro:config:setup': ({ addWatchFile }) => {
				writeReadmePages();
				addWatchFile(readme);
			},
		},
	};
}
