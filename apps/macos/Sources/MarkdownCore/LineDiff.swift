import Foundation

/// One contiguous change between an original and a modified text, in line units.
public struct DiffHunk: Equatable, Identifiable, Sendable {
    public var id: Int
    /// Index into the original lines where the removed block starts.
    public var originalStart: Int
    public var removed: [String]
    /// Index into the modified lines where the added block starts.
    public var modifiedStart: Int
    public var added: [String]

    public init(id: Int, originalStart: Int, removed: [String], modifiedStart: Int, added: [String]) {
        self.id = id
        self.originalStart = originalStart
        self.removed = removed
        self.modifiedStart = modifiedStart
        self.added = added
    }
}

public enum HunkDecision: String, Codable, Sendable {
    case pending, accepted, rejected
}

/// Line-level diff (Myers-style LCS) with hunk resolution and word-level highlights.
public enum LineDiff {
    public static func lines(_ s: String) -> [String] {
        s.components(separatedBy: "\n")
    }

    /// Longest-common-subsequence edit script on arrays of hashable tokens.
    /// Returns pairs (i, j) of matching indices in increasing order.
    public static func lcsMatches<T: Hashable>(_ a: [T], _ b: [T]) -> [(Int, Int)] {
        // Trim common prefix/suffix for speed.
        var prefix = 0
        while prefix < a.count && prefix < b.count && a[prefix] == b[prefix] { prefix += 1 }
        var suffix = 0
        while suffix < a.count - prefix && suffix < b.count - prefix && a[a.count - 1 - suffix] == b[b.count - 1 - suffix] { suffix += 1 }
        let a2 = Array(a[prefix..<(a.count - suffix)])
        let b2 = Array(b[prefix..<(b.count - suffix)])
        var result: [(Int, Int)] = (0..<prefix).map { ($0, $0) }
        result += myers(a2, b2).map { ($0.0 + prefix, $0.1 + prefix) }
        result += (0..<suffix).map { (a.count - suffix + $0, b.count - suffix + $0) }
        return result
    }

    /// Myers O(ND) diff returning matched index pairs.
    static func myers<T: Hashable>(_ a: [T], _ b: [T]) -> [(Int, Int)] {
        let n = a.count, m = b.count
        if n == 0 || m == 0 { return [] }
        let max = n + m
        var v = [Int](repeating: 0, count: 2 * max + 2)
        var trace: [[Int]] = []
        outer: for d in 0...max {
            trace.append(v)
            var k = -d
            while k <= d {
                var x: Int
                if k == -d || (k != d && v[k - 1 + max] < v[k + 1 + max]) {
                    x = v[k + 1 + max]
                } else {
                    x = v[k - 1 + max] + 1
                }
                var y = x - k
                while x < n && y < m && a[x] == b[y] { x += 1; y += 1 }
                v[k + max] = x
                if x >= n && y >= m { trace.append(v); break outer }
                k += 2
            }
        }
        // Backtrack.
        var matches: [(Int, Int)] = []
        var x = n, y = m
        for d in stride(from: trace.count - 2, through: 0, by: -1) {
            let vv = trace[d]
            let k = x - y
            let prevK: Int
            if k == -d || (k != d && vv[k - 1 + max] < vv[k + 1 + max]) { prevK = k + 1 } else { prevK = k - 1 }
            let prevX = vv[prevK + max]
            let prevY = prevX - prevK
            while x > prevX && y > prevY { x -= 1; y -= 1; matches.append((x, y)) }
            if d > 0 { x = prevX; y = prevY }
        }
        return matches.reversed()
    }

    /// Computes hunks between two texts.
    public static func hunks(original: String, modified: String) -> [DiffHunk] {
        hunks(originalLines: lines(original), modifiedLines: lines(modified))
    }

    public static func hunks(originalLines a: [String], modifiedLines b: [String]) -> [DiffHunk] {
        let matches = lcsMatches(a, b)
        var result: [DiffHunk] = []
        var i = 0, j = 0
        for (mi, mj) in matches + [(a.count, b.count)] {
            if mi > i || mj > j {
                result.append(DiffHunk(id: result.count, originalStart: i, removed: Array(a[i..<mi]), modifiedStart: j, added: Array(b[j..<mj])))
            }
            i = mi + 1; j = mj + 1
        }
        return result
    }

    /// Builds the text that results from applying decisions to hunks.
    /// Accepted and pending hunks keep the modified text; rejected hunks restore the original.
    public static func resolve(original: String, modified: String, hunks: [DiffHunk], decisions: [Int: HunkDecision]) -> String {
        let a = lines(original), b = lines(modified)
        var out: [String] = []
        var j = 0
        for h in hunks {
            // Unchanged lines in modified before this hunk.
            out += b[j..<h.modifiedStart]
            if decisions[h.id] == .rejected { out += h.removed } else { out += h.added }
            j = h.modifiedStart + h.added.count
        }
        out += b[j..<b.count]
        _ = a
        return out.joined(separator: "\n")
    }

    // MARK: Word-level

    public enum Segment: Equatable, Sendable {
        case same(String), removed(String), added(String)
    }

    static func tokenize(_ s: String) -> [String] {
        var tokens: [String] = []
        var cur = ""
        var curIsWord: Bool? = nil
        for ch in s {
            let isWord = ch.isLetter || ch.isNumber || ch == "_"
            if isWord, curIsWord == true { cur.append(ch); continue }
            if !cur.isEmpty { tokens.append(cur) }
            cur = String(ch)
            curIsWord = isWord
        }
        if !cur.isEmpty { tokens.append(cur) }
        return tokens
    }

    /// Word-level segments for a pair of changed lines.
    public static func wordDiff(old: String, new: String) -> (old: [Segment], new: [Segment]) {
        let a = tokenize(old), b = tokenize(new)
        let matches = lcsMatches(a, b)
        var oldSegs: [Segment] = [], newSegs: [Segment] = []
        var i = 0, j = 0
        func push(_ arr: inout [Segment], _ seg: Segment) {
            if let last = arr.last {
                switch (last, seg) {
                case let (.same(x), .same(y)): arr[arr.count - 1] = .same(x + y); return
                case let (.removed(x), .removed(y)): arr[arr.count - 1] = .removed(x + y); return
                case let (.added(x), .added(y)): arr[arr.count - 1] = .added(x + y); return
                default: break
                }
            }
            arr.append(seg)
        }
        for (mi, mj) in matches + [(a.count, b.count)] {
            for k in i..<mi { push(&oldSegs, .removed(a[k])) }
            for k in j..<mj { push(&newSegs, .added(b[k])) }
            if mi < a.count && mj < b.count {
                push(&oldSegs, .same(a[mi])); push(&newSegs, .same(b[mj]))
            }
            i = mi + 1; j = mj + 1
        }
        return (oldSegs, newSegs)
    }

    /// Counts of added and removed lines (for "+12 −3" summaries).
    public static func stat(original: String, modified: String) -> (added: Int, removed: Int) {
        let hs = hunks(original: original, modified: modified)
        return (hs.reduce(0) { $0 + $1.added.count }, hs.reduce(0) { $0 + $1.removed.count })
    }
}
