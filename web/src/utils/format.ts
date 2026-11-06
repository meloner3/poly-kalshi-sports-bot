export function formatPrice(price: number): string {
  return `${(price * 100).toFixed(3)}¢`;
}

export function formatProfit(profit: number): string {
  return `$${profit.toFixed(2)}`;
}

export function formatPercent(percent: number): string {
  return `${percent.toFixed(2)}%`;
}

export function formatDateTime(dateString?: string): string {
  if (!dateString) return 'Unknown';
  const date = new Date(dateString);
  return date.toLocaleString('en-US', {
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
  });
}

export function truncateString(str: string, maxLen: number): string {
  if (str.length <= maxLen) return str;
  return str.substring(0, maxLen - 2) + '..';
}

export function getArbitrageTypeLabel(type: string): string {
  switch (type) {
    case 'KalshiYesPolymarketNo':
      return 'K Buy Yes + P Buy No';
    case 'KalshiNoPolymarketYes':
      return 'K Buy No + P Buy Yes';
    default:
      return type;
  }
}
