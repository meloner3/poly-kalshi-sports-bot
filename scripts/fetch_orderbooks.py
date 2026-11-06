#!/usr/bin/env python3
"""
Fetch order-book data for Polymarket NBA events.
Used to analyze order-book ordering and structure.
"""

import requests
import json
from datetime import datetime
from pathlib import Path

# Polymarket API endpoints
GAMMA_API = "https://gamma-api.polymarket.com"
CLOB_API = "https://clob.polymarket.com"

def get_nba_markets():
    """Fetch all NBA markets."""
    print("📊 Fetching the NBA market list...")
    
    # Method 1: Fetch through search.
    url = f"{GAMMA_API}/markets"
    params = {
        "limit": 100,
        "closed": "false",  # Only open markets.
    }
    
    try:
        response = requests.get(url, params=params, timeout=10)
        response.raise_for_status()
        all_markets = response.json()
    except Exception as e:
        print(f"   ❌ Failed to fetch the market list: {e}")
        return []
    
    # Filter NBA markets.
    nba_markets = []
    for market in all_markets:
        question = market.get("question", "").lower()
        description = market.get("description", "").lower()
        tags = [tag.lower() for tag in market.get("tags", [])]
        
        # Check for NBA-related keywords.
        if any(keyword in question or keyword in description or keyword in str(tags) 
               for keyword in ["nba", "basketball", "lakers", "warriors", "celtics", "heat"]):
            nba_markets.append(market)
    
    print(f"   Found {len(nba_markets)} NBA-related markets")
    
    # If none are found, try known token IDs extracted from logs.
    if len(nba_markets) == 0:
        print("   ℹ️  Testing with known NBA token IDs...")
        # Token IDs extracted from logs.
        test_tokens = [
            "94515776290373751754638142228993059501097351216445649452643423016914071837398",  # CHI-MIA
            "16215889044933102237616087156593010997453882627000153373435840826284741185376",  # CHI-MIA
            "103007116798336628661619679985222919791811823436106325887661137036158976283917", # NYK-POR
        ]
        
        # Create mock market objects for testing.
        for i, token_id in enumerate(test_tokens):
            nba_markets.append({
                "question": f"Test NBA Market {i+1}",
                "id": f"test_{i}",
                "tokens": [{"token_id": token_id, "outcome": "Test"}]
            })
    
    return nba_markets

def get_orderbook(token_id):
    """Fetch the order book for the specified token."""
    url = f"{CLOB_API}/book"
    params = {"token_id": token_id}
    
    try:
        response = requests.get(url, params=params, timeout=10)
        response.raise_for_status()
        return response.json()
    except Exception as e:
        print(f"   ❌ Failed to fetch the order book: {e}")
        return None

def analyze_orderbook(orderbook_data, token_id):
    """Analyze order-book data."""
    if not orderbook_data:
        return None
    
    bids = orderbook_data.get("bids", [])
    asks = orderbook_data.get("asks", [])
    
    analysis = {
        "token_id": token_id,
        "timestamp": datetime.now().isoformat(),
        "bids_count": len(bids),
        "asks_count": len(asks),
        "bids": [],
        "asks": [],
    }
    
    # Analyze bids.
    if bids:
        for i, bid in enumerate(bids[:5]):  # Use only the first five.
            price = float(bid.get("price", 0))
            size = float(bid.get("size", 0))
            analysis["bids"].append({
                "index": i,
                "price": price,
                "size": size,
                "value": price * size
            })
        
        # Check ordering.
        prices = [float(b.get("price", 0)) for b in bids]
        is_ascending = all(prices[i] <= prices[i+1] for i in range(len(prices)-1))
        is_descending = all(prices[i] >= prices[i+1] for i in range(len(prices)-1))
        
        analysis["bids_sort"] = "ascending" if is_ascending else ("descending" if is_descending else "unsorted")
        analysis["best_bid"] = {"price": prices[0], "position": "first"} if prices else None
        analysis["worst_bid"] = {"price": prices[-1], "position": "last"} if prices else None
    
    # Analyze asks.
    if asks:
        for i, ask in enumerate(asks[:5]):  # Use only the first five.
            price = float(ask.get("price", 0))
            size = float(ask.get("size", 0))
            analysis["asks"].append({
                "index": i,
                "price": price,
                "size": size,
                "value": price * size
            })
        
        # Check ordering.
        prices = [float(a.get("price", 0)) for a in asks]
        is_ascending = all(prices[i] <= prices[i+1] for i in range(len(prices)-1))
        is_descending = all(prices[i] >= prices[i+1] for i in range(len(prices)-1))
        
        analysis["asks_sort"] = "ascending" if is_ascending else ("descending" if is_descending else "unsorted")
        analysis["best_ask"] = {"price": prices[0], "position": "first"} if prices else None
        analysis["worst_ask"] = {"price": prices[-1], "position": "last"} if prices else None
    
    return analysis

def main():
    print("=" * 60)
    print("Polymarket NBA Order-Book Data Collection")
    print("=" * 60)
    print()
    
    # Create the output directory.
    output_dir = Path("orderbook_analysis")
    output_dir.mkdir(exist_ok=True)
    
    # Fetch NBA markets.
    markets = get_nba_markets()
    
    if not markets:
        print("❌ No NBA markets found")
        return
    
    # Collect all order-book data.
    all_orderbooks = []
    all_analysis = []
    
    print(f"\n📥 Fetching order-book data...")
    print()
    
    for i, market in enumerate(markets[:10], 1):  # Limit to the first 10 markets.
        question = market.get("question", "Unknown")
        market_id = market.get("id", "")
        tokens = market.get("tokens", [])
        
        print(f"{i}. {question}")
        print(f"   Market ID: {market_id}")
        
        for token in tokens:
            token_id = token.get("token_id", "")
            outcome = token.get("outcome", "Unknown")
            
            if not token_id:
                continue
            
            print(f"   📖 Fetching the order book for {outcome}...")
            
            orderbook = get_orderbook(token_id)
            if orderbook:
                all_orderbooks.append({
                    "market": question,
                    "market_id": market_id,
                    "outcome": outcome,
                    "token_id": token_id,
                    "orderbook": orderbook
                })
                
                # Analyze the order book.
                analysis = analyze_orderbook(orderbook, token_id)
                if analysis:
                    analysis["market"] = question
                    analysis["outcome"] = outcome
                    all_analysis.append(analysis)
                    
                    # Print a summary.
                    if analysis.get("bids"):
                        print(f"      Bids: {analysis['bids_sort']}, "
                              f"first={analysis['bids'][0]['price']:.4f}, "
                              f"last={analysis['bids'][-1]['price']:.4f}")
                    if analysis.get("asks"):
                        print(f"      Asks: {analysis['asks_sort']}, "
                              f"first={analysis['asks'][0]['price']:.4f}, "
                              f"last={analysis['asks'][-1]['price']:.4f}")
        
        print()
    
    # Save raw data.
    timestamp = datetime.now().strftime("%Y%m%d_%H%M%S")
    
    raw_file = output_dir / f"orderbooks_raw_{timestamp}.json"
    with open(raw_file, "w", encoding="utf-8") as f:
        json.dump(all_orderbooks, f, indent=2, ensure_ascii=False)
    print(f"✅ Raw data saved: {raw_file}")
    
    # Save analysis results.
    analysis_file = output_dir / f"orderbooks_analysis_{timestamp}.json"
    with open(analysis_file, "w", encoding="utf-8") as f:
        json.dump(all_analysis, f, indent=2, ensure_ascii=False)
    print(f"✅ Analysis results saved: {analysis_file}")
    
    # Generate a summary report.
    print("\n" + "=" * 60)
    print("📊 Order-Book Ordering Analysis Summary")
    print("=" * 60)
    
    bids_sort_summary = {}
    asks_sort_summary = {}
    
    for analysis in all_analysis:
        bids_sort = analysis.get("bids_sort", "unknown")
        asks_sort = analysis.get("asks_sort", "unknown")
        
        bids_sort_summary[bids_sort] = bids_sort_summary.get(bids_sort, 0) + 1
        asks_sort_summary[asks_sort] = asks_sort_summary.get(asks_sort, 0) + 1
    
    print(f"\nBid ordering statistics:")
    for sort_type, count in bids_sort_summary.items():
        print(f"  {sort_type}: {count} markets")
    
    print(f"\nAsk ordering statistics:")
    for sort_type, count in asks_sort_summary.items():
        print(f"  {sort_type}: {count} markets")
    
    # Save the summary.
    summary = {
        "timestamp": datetime.now().isoformat(),
        "total_markets": len(markets),
        "analyzed_markets": len(all_analysis),
        "bids_sort_summary": bids_sort_summary,
        "asks_sort_summary": asks_sort_summary,
    }
    
    summary_file = output_dir / f"summary_{timestamp}.json"
    with open(summary_file, "w", encoding="utf-8") as f:
        json.dump(summary, f, indent=2, ensure_ascii=False)
    print(f"\n✅ Summary report saved: {summary_file}")
    
    print("\n" + "=" * 60)
    print("✅ Data collection complete!")
    print("=" * 60)

if __name__ == "__main__":
    main()
