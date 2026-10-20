import assert from 'node:assert/strict';
import { test } from 'node:test';
import { anchorMap, sections } from './readme-pages.mjs';

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
