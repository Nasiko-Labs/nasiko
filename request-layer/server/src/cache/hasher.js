const crypto = require("crypto");

/**
 * RequestHasher — Generates deterministic cache keys from request data.
 *
 * Identical requests (same agent + query + params) always produce the same key,
 * regardless of session, user, or timestamp.
 */
class RequestHasher {
  /**
   * Compute a cache key from request components.
   * @param {string} agentName - Target agent name
   * @param {string} query - User query text
   * @param {object} [params={}] - Additional parameters
   * @returns {string} Cache key in format `cache:{agentName}:{hash}`
   */
  static computeKey(agentName, query, params = {}) {
    // Normalize inputs for consistency
    const normalizedAgent = agentName.trim().toLowerCase();
    const normalizedQuery = query.trim().toLowerCase();

    // Sort params for deterministic ordering
    const sortedParams = Object.keys(params)
      .sort()
      .reduce((acc, key) => {
        acc[key] = params[key];
        return acc;
      }, {});

    // Build the content to hash
    const content = JSON.stringify({
      agent: normalizedAgent,
      query: normalizedQuery,
      params: sortedParams,
    });

    // SHA-256 hash
    const hash = crypto.createHash("sha256").update(content).digest("hex");

    return `cache:${normalizedAgent}:${hash}`;
  }

  /**
   * Extract the agent name from a cache key.
   * @param {string} cacheKey
   * @returns {string} Agent name
   */
  static extractAgent(cacheKey) {
    const parts = cacheKey.split(":");
    return parts.length >= 2 ? parts[1] : "unknown";
  }
}

module.exports = RequestHasher;
