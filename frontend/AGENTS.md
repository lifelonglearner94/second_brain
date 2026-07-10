# Frontend

## Commands

- `npm run dev` - Vite dev server
- `npm run build` - `adapter-static` PWA Bundle into `build/`
- `npm run preview` - serve the built PWA Bundle
- `npm test` - Vitest unit/component tests
- `npm run test:e2e` - Playwright smoke tests (builds first)
- `npm run check` - `svelte-check` typecheck
- `npm run lint` - ESLint

## Stack

SvelteKit (Svelte 5, runes) + `adapter-static`. Vitest + `@testing-library/svelte` for unit/component. Playwright for e2e. The PWA Bundle is a static artifact baked into the Edge image (infra CONTEXT).
