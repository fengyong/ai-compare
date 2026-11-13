import type { Extension } from '@codemirror/state';
import type { LangId } from './langDetect';

/**
 * Lezer 语言包按需加载（FR-TXT-013/014 简化版，常用集）。
 *
 * 动态 import → Vite 自动分包：未用到的语言包不进主 bundle；
 * 无语言包（detectLangId 为 null 或加载失败）回退纯文本。
 * 加载结果按语言缓存（同一会话不重复请求）。
 */

type Loader = () => Promise<Extension> | Extension;

const LOADERS: Record<LangId, Loader> = {
  javascript: () => import('@codemirror/lang-javascript').then((m) => m.javascript()),
  typescript: () => import('@codemirror/lang-javascript').then((m) => m.javascript({ typescript: true })),
  json: () => import('@codemirror/lang-json').then((m) => m.json()),
  markdown: () => import('@codemirror/lang-markdown').then((m) => m.markdown()),
  python: () => import('@codemirror/lang-python').then((m) => m.python()),
  rust: () => import('@codemirror/lang-rust').then((m) => m.rust()),
};

const cache = new Map<LangId, Extension>();

/** 加载语言扩展（命中缓存即同步返回；失败/无包返回 null = 纯文本） */
export async function loadLangExtension(id: LangId | null): Promise<Extension | null> {
  if (!id) return null;
  const hit = cache.get(id);
  if (hit) return hit;
  try {
    const ext = await LOADERS[id]();
    cache.set(id, ext);
    return ext;
  } catch {
    return null;
  }
}

/** 已缓存的语言（同步，供测试/诊断） */
export function cachedLangs(): LangId[] {
  return [...cache.keys()];
}
