import assert from 'node:assert/strict';
import { test } from 'node:test';
import { anchorMap, sectionMarkdown, sections } from './readme-pages.mjs';

test('a `## ` line inside an indented or tilde fence is not a section', () => {
	const readme = ['## Install', '- Run:', '  ```sh', '## not a heading', '  ```', '~~~', '## nor this', '```', '~~~', '## Keys'].join('\n');
	assert.deepEqual(
		sections(readme).map((s) => s.title),
		['Install', 'Keys'],
	);
});

test('a repeated subheading links to the anchor its own page gives it', () => {
	const all = sections(['## Install', '### Notes', '## Keys', '### Notes'].join('\n'));
	const pages = [
		{ title: 'Install', slug: 'install' },
		{ title: 'Keys', slug: 'keys' },
	];
	const anchors = anchorMap(all, pages);
	assert.deepEqual(anchors.get('notes'), { page: 'install', hash: 'notes' });
	assert.deepEqual(anchors.get('notes-1'), { page: 'keys', hash: 'notes' });
});

test('a raw <img> of a repo file fails, one in a code span does not', () => {
	const [page] = sections(['## Install', '`<img src="a.png">`', '<img src="https://example.com/a.png">'].join('\n'));
	assert.doesNotThrow(() => sectionMarkdown({ ...page, slug: 'install' }, new Map(), '..'));
	page.lines.push({ line: '<img src="docs/screenshots/a.png" alt="">', fenced: false });
	assert.throws(() => sectionMarkdown({ ...page, slug: 'install' }, new Map(), '..'), /<img>/);
});

test('links and images are rewritten outside code spans, keeping titles', () => {
	const [page] = sections(
		[
			'## Install',
			'![Icon](assets/icon.svg "The icon") and ![Shot](docs/screenshots/a.png)',
			'See [`docs/a.md`](docs/a.md "ADR") and `[x](docs/b.md)`.',
			'[Keys](#keys), [web](https://example.com) and [up](../x).',
		].join('\n'),
	);
	const anchors = new Map([['keys', { page: 'keys' }]]);
	assert.equal(
		sectionMarkdown({ ...page, slug: 'install' }, anchors, '../..'),
		[
			'![Icon](../../assets/icon.svg "The icon") and ![Shot](../../docs/screenshots/a.png)',
			'See [`docs/a.md`](https://github.com/emzbtw/vidicon/blob/main/docs/a.md "ADR") and `[x](docs/b.md)`.',
			'[Keys](/vidicon/docs/keys/), [web](https://example.com) and [up](../x).',
		].join('\n'),
	);
});
