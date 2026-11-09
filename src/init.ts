import fs from 'fs-extra';
import path from 'path';
import { fileURLToPath } from 'url';
import chalk from 'chalk';

const ROLES = ['pm', 'dev', 'ui', 'tester', 'admin'];

const RUNTIME_DIRS = [
  'communication/bus/{role}/inbox',
  'communication/bus/{role}/active',
  'workspace',
  '.artifacts',
  'memory_center/archive',
  'memory_center/project_history',
];

function scaffoldRoot(): string {
  const __dirname = path.dirname(fileURLToPath(import.meta.url));
  return path.join(__dirname, '..', 'scaffold');
}

export const TEAM_DIR = '.opc_agent';

export function initProject(target: string) {
  const projectDir = path.resolve(target);
  const targetRoot = path.join(projectDir, TEAM_DIR);
  const scaffold = scaffoldRoot();

  if (!fs.existsSync(scaffold)) {
    console.log(chalk.red('Error: scaffold data not found in package.'));
    return;
  }

  const created: string[] = [];
  const skipped: string[] = [];

  const allFiles = getAllFiles(scaffold).filter(f => !path.basename(f).startsWith('.'));
  for (const srcFile of allFiles.sort()) {
    const rel = path.relative(scaffold, srcFile);
    const dst = path.join(targetRoot, rel);
    if (fs.existsSync(dst)) {
      skipped.push(rel);
      continue;
    }
    fs.ensureDirSync(path.dirname(dst));
    fs.copyFileSync(srcFile, dst);
    created.push(rel);
  }

  const opcJson = path.join(targetRoot, 'opc.json');
  if (!fs.existsSync(opcJson)) {
    const example = path.join(targetRoot, 'opc.json.example');
    if (fs.existsSync(example)) {
      fs.copyFileSync(example, opcJson);
      created.push('opc.json');
    }
  }

  for (const pattern of RUNTIME_DIRS) {
    if (pattern.includes('{role}')) {
      for (const role of ROLES) {
        fs.ensureDirSync(path.join(targetRoot, pattern.replace('{role}', role)));
      }
    } else {
      fs.ensureDirSync(path.join(targetRoot, pattern));
    }
  }

  console.log(`\n${chalk.bold.green(`OPC initialized in ${targetRoot}`)}\n`);
  if (created.length > 0) {
    for (const f of created) {
      console.log(`  ${chalk.green('+')} ${path.join(TEAM_DIR, f)}`);
    }
  }
  if (skipped.length > 0) {
    console.log(`\n  ${chalk.dim(`${skipped.length} existing files unchanged`)}`);
  }
  console.log(`\n${chalk.bold('Next steps:')}`);
  console.log(`  1. Edit ${chalk.cyan(path.join(TEAM_DIR, 'opc.json'))} — set your api_key`);
  console.log(`  2. Run ${chalk.cyan('opc')} to start\n`);
}

function getAllFiles(dir: string): string[] {
  const results: string[] = [];
  const entries = fs.readdirSync(dir, { withFileTypes: true });
  for (const entry of entries) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      results.push(...getAllFiles(full));
    } else {
      results.push(full);
    }
  }
  return results;
}
