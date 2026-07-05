export type TabDef = {
	href: string;
	label: string;
	slug: string;
};

export const APP_TABS: readonly TabDef[] = [
	{ href: '/app', label: 'Capture', slug: 'capture' },
	{ href: '/app/graph', label: 'Graph', slug: 'graph' },
	{ href: '/app/chat', label: 'Chat', slug: 'chat' }
] as const;

export function isTabActive(href: string, pathname: string): boolean {
	if (href === '/app') {
		return pathname === '/app';
	}
	return pathname === href || pathname.startsWith(href + '/');
}
