#!/usr/bin/env node
/**
 * `dbpylot` command for the npm package.
 *
 * Registered as the `dbpylot` bin in package.json. Runs the real CLI in-process
 * via the native addon — no separate Rust binary is required. Pass a stable
 * program name as argv[0] so help/usage reads "dbpylot".
 */

const { runCli } = require('../index.js')

process.exit(runCli(['dbpylot', ...process.argv.slice(2)]))
