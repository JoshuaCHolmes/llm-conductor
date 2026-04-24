# Capturing Outlier Conversation Creation

## Quick Method (5 minutes):

1. **Open Outlier Playground** in your browser
   - Go to: https://playground.outlier.ai
   
2. **Open DevTools Network Tab**
   - Press `F12`
   - Click **Network** tab
   - Check **"Preserve log"** checkbox
   - Clear the log (trash icon)

3. **Create a New Conversation**
   - Click "New Chat" or whatever button starts a fresh conversation
   - OR just type a message if you're on a fresh page

4. **Find the Request**
   - Look for requests in the Network tab
   - Filter by: `conversations` or `turn`
   - Look for POST requests

5. **Copy the Request Details**
   - Right-click on the request
   - Select **"Copy" → "Copy as cURL"**
   - Paste it here or into a file

This will give us the EXACT request including all headers, body, etc.

---

Alternatively, can you just:
1. Open browser
2. Go to playground.outlier.ai  
3. Press F12 → Network tab → Preserve Log
4. Start a new chat
5. Copy the curl command for any request to `conversations` endpoint

Then paste it here!
