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

const siteDir = dirname(fileURLToPath(import.meta.url));
const readme = join(siteDir, '..', 'README.md');
const outDir = join(siteDir, 'src', 'content', 'docs', 'docs');

// Split the README into `## ` sections, skipping fenced code blocks.
// The HTML header and intro come before the first `## ` and are dropped.
function sections(text) {
	const out = [];
	let fenced = false;
	for (const line of text.split('\n')) {
		if (line.startsWith('```')) fenced = !fenced;
		if (!fenced && line.startsWith('## ')) out.push({ title: line.slice(3).trim(), lines: [] });
		else if (out.length) out.at(-1).lines.push({ line, fenced: fenced || line.startsWith('```') });
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

export function writeReadmePages() {
	const all = sections(readFileSync(readme, 'utf8'));
	const pages = PAGES.map((title) => {
		const section = all.find((s) => s.title === title);
		if (!section) throw new Error(`README.md has no "## ${title}" section`);
		return { ...section, slug: new GithubSlugger().slug(title) };
	});

	// Where each README anchor lives on the site: a section is its page,
	// a subheading is an anchor on its section's page.
	const anchors = new Map();
	const slugger = new GithubSlugger();
	for (const s of all) {
		const page = pages.find((p) => p.title === s.title);
		const anchor = slugger.slug(s.title);
		if (page) anchors.set(anchor, { page: page.slug });
		for (const { line, fenced } of s.lines) {
			const h = !fenced && line.match(/^#{3,} (.*)/);
			if (h) {
				const sub = slugger.slug(h[1].trim());
				if (page) anchors.set(sub, { page: page.slug, hash: sub });
			}
		}
	}

	rmSync(outDir, { recursive: true, force: true });
	mkdirSync(outDir, { recursive: true });
	const repoRoot = relative(outDir, join(siteDir, '..'));
	pages.forEach((page, i) => {
		const body = page.lines
			.map(({ line, fenced }) => {
				if (fenced) return line;
				return (
					line
						// The page title is the h1, so `###` becomes `##`.
						.replace(/^#(#{2,}) /, '$1 ')
						.replace(/\]\(#([^)]+)\)/g, (match, anchor) => {
							const target = anchors.get(anchor);
							if (!target) return match;
							if (target.page === page.slug && target.hash) return `](#${target.hash})`;
							return `](/vidicon/docs/${target.page}/${target.hash ? `#${target.hash}` : ''})`;
						})
						.replace(/\]\((docs\/screenshots\/[^)]+)\)/g, (_, path) => `](${repoRoot}/${path})`)
				);
			})
			.join('\n')
			.trim();
		const frontmatter = `---\ntitle: ${JSON.stringify(page.title)}\nsidebar:\n  order: ${i + 1}\n---\n\n`;
		writeFileSync(join(outDir, `${page.slug}.md`), `${frontmatter}<!-- Generated from README.md by site/readme-pages.mjs. Edit the README. -->\n\n${body}\n`);
	});
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
