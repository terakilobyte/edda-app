// Cross-tab deep links into Help: any panel calls openHelp("route-plotting")
// and App switches to the Help tab scrolled to that topic.
export const help = $state({ topic: null, requested: 0 });

export function openHelp(topic) {
  help.topic = topic;
  help.requested++;
}
