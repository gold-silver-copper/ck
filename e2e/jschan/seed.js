// What ck's account test (src/post/jschan_e2e.rs) needs on a fresh jschan or ptchina, made
// with the server's own code (run from servers.sh, the server's folder as the argument):
//
//   node seed.js accounts <server>          before it starts: the accounts, the settings
//   node seed.js boards <server> <url>      once it answers: the boards
//
// The names, password and two-factor secret are the test's too: change them together.
'use strict';

const path = require('path');
const fs = require('fs');

const [, , phase, dir, url] = process.argv;
const app = (m) => require(path.join(dir, m));
const PASSWORD = 'ck-e2e-password';
// Base32, as jschan keeps it (its codes are TOTP with SHA-256, 6 digits, 30 seconds).
const TWOFACTOR = 'GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ';
// The account that makes the boards (only ROOT may make more than one on ptchina).
const SEEDER = 'ckseeder';

const accounts = async () => {
	const Mongo = app('db/db.js');
	await Mongo.connect();
	const { Accounts } = app('db/');
	const roleManager = app('lib/permission/rolemanager.js');
	await roleManager.load();
	const Permission = app('lib/permission/permission.js');
	const { Permissions } = app('lib/permission/permissions.js');
	const { ANON, ROOT } = roleManager.roles;
	await Accounts.insertOne(SEEDER, SEEDER, PASSWORD, ROOT);
	await Accounts.insertOne('ckplain', 'ckplain', PASSWORD, ANON);
	await Accounts.insertOne('cktwofa', 'cktwofa', PASSWORD, ANON);
	await Accounts.updateTwofactor('cktwofa', TWOFACTOR);
	// Posts without a captcha: what ptchan's gold (donor) accounts may do, with the donor bit
	// itself where the server has one.
	const gold = new Permission(ANON.base64);
	gold.set(Permissions.BYPASS_CAPTCHA);
	if (Permissions.DONOR != null) {
		gold.set(Permissions.DONOR);
	}
	await Accounts.insertOne('ckgold', 'ckgold', PASSWORD, gold);
	// Posting as fast as a test does, and no captcha switched on by the posts' pace.
	const config = await Mongo.getConfig();
	config.floodTimers = { sameContentSameIp: 0, sameContentAnyIp: 0, anyContentSameIp: 0 };
	await Mongo.setConfig(config);
	// ck-web opens a site from its robots.txt, which nginx serves in production.
	fs.writeFileSync(path.join(dir, 'static', 'robots.txt'), 'User-agent: *\nDisallow:\n');
};

// Log in as the seeder, and make a board with captchas off (`open`) and one with them on
// for every post (`capt`).
const boards = async () => {
	const form = (fields) => new URLSearchParams(fields);
	const login = await fetch(`${url}/forms/login`, { method: 'POST', redirect: 'manual', headers: { 'x-using-xhr': 'true' }, body: form({ username: SEEDER, password: PASSWORD }) });
	const cookie = (login.headers.get('set-cookie') || '').split(';')[0];
	if (login.status !== 302 || !cookie) {
		throw new Error(`the seeder couldn't log in: ${login.status} ${await login.text()}`);
	}
	for (const uri of ['open', 'capt']) {
		const r = await fetch(`${url}/forms/create`, { method: 'POST', redirect: 'manual', headers: { 'x-using-xhr': 'true', cookie }, body: form({ uri, name: uri, description: 'ck test' }) });
		if (r.status !== 302) {
			throw new Error(`couldn't make /${uri}/: ${r.status} ${await r.text()}`);
		}
	}
	const Mongo = app('db/db.js');
	await Mongo.connect();
	const cache = app('lib/redis/redis.js');
	for (const [uri, mode] of [['open', 0], ['capt', 2]]) {
		// The triggers would turn captchas on by themselves after a few posts.
		await Mongo.db.collection('boards').updateOne({ _id: uri }, { $set: { 'settings.captchaMode': mode, 'settings.tphTrigger': 0, 'settings.pphTrigger': 0 } });
		await cache.del(`board:${uri}`);
	}
	await fetch(`${url}/forms/logout`, { method: 'POST', redirect: 'manual', headers: { cookie } });
};

({ accounts, boards })[phase]().then(() => process.exit(0), (e) => {
	console.error(e);
	process.exit(1);
});
