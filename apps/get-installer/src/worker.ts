// Cloudflare Worker — proxies `scripts/install.sh` from the GitHub raw URL
// behind https://get.oxiremote.erai.dev so users can pipe it:
//
//   curl -fsSL https://get.oxiremote.erai.dev | bash
//
// We always serve the script from a pinned branch (default `main`) so updates
// to install.sh ship without redeploying this Worker. Edge caches for 5 min.

interface Env {
  GITHUB_REPO?: string
  GITHUB_REF?: string
  CACHE_TTL_SECONDS?: string
}

const DEFAULT_REPO = 'nhtera/oxiremote'
const DEFAULT_REF = 'main'
const DEFAULT_CACHE_TTL = 300

export default {
  async fetch(req: Request, env: Env, ctx: ExecutionContext): Promise<Response> {
    const url = new URL(req.url)
    if (req.method !== 'GET' && req.method !== 'HEAD') {
      return new Response('method not allowed', { status: 405 })
    }
    if (url.pathname !== '/' && url.pathname !== '/install.sh') {
      return new Response('not found', { status: 404 })
    }

    const repo = env.GITHUB_REPO || DEFAULT_REPO
    const ref = env.GITHUB_REF || DEFAULT_REF
    const ttl = Number(env.CACHE_TTL_SECONDS) || DEFAULT_CACHE_TTL
    const upstream = `https://raw.githubusercontent.com/${repo}/${ref}/scripts/install.sh`

    const cache = caches.default
    const cacheKey = new Request(upstream, { method: 'GET' })
    let res = await cache.match(cacheKey)
    if (!res) {
      const upstreamRes = await fetch(upstream, {
        cf: { cacheTtl: ttl, cacheEverything: true },
      })
      if (!upstreamRes.ok) {
        return new Response(`upstream ${upstreamRes.status} fetching install.sh`, {
          status: 502,
        })
      }
      res = new Response(upstreamRes.body, upstreamRes)
      res.headers.set('content-type', 'text/x-shellscript; charset=utf-8')
      res.headers.set('cache-control', `public, max-age=${ttl}`)
      res.headers.set('x-source', upstream)
      ctx.waitUntil(cache.put(cacheKey, res.clone()))
    }
    return res
  },
}
