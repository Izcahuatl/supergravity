local b, c, f = string.byte, string.char, math.floor
local unp = table.unpack or unpack

table.move = table.move or function(a1, f, e, t, a2)
    a2 = a2 or a1
    if e >= f then
        if t > e or t <= f or a1 ~= a2 then
            for i = 0, e - f do a2[t + i] = a1[f + i] end
        else
            for i = e - f, 0, -1 do a2[t + i] = a1[f + i] end
        end
    end
    return a2
end

math.maxinteger, math.mininteger = 9007199254740991, -9007199254740991

local function is_int(x)
    return type(x) == "number" and x % 1 == 0 and x >= -9007199254740991 and x <= 9007199254740991
end
math.tointeger = math.tointeger or function(x) local n = tonumber(x) return is_int(n) and n or nil end
math.type = math.type or function(x) return is_int(x) and "integer" or (type(x) == "number" and "float" or nil) end
math.ult = math.ult or function(m, n) return m < n end

utf8 = utf8 or {}
utf8.charpattern = "[%z\x01-\x7F\xC2-\xF4][\x80-\xBF]*"

local function dec(s, p)
    local b1 = b(s, p) if not b1 then return nil end
    if b1 < 128 then return p + 1, b1 end
    local b2 = b(s, p + 1) if not b2 or b2 < 128 or b2 > 191 then return false end
    if b1 < 224 then return p + 2, (b1 - 192) * 64 + (b2 - 128) end
    local b3 = b(s, p + 2) if not b3 or b3 < 128 or b3 > 191 then return false end
    if b1 < 240 then return p + 3, (b1 - 224) * 4096 + (b2 - 128) * 64 + (b3 - 128) end
    local b4 = b(s, p + 3) if not b4 or b4 < 128 or b4 > 191 then return false end
    if b1 < 248 then return p + 4, (b1 - 240) * 262144 + (b2 - 128) * 4096 + (b3 - 128) * 64 + (b4 - 128) end
    return false
end

local function bounds(s, i, j)
    local n = #s
    i, j = i or 1, j or n
    return i < 0 and n + i + 1 or i, j < 0 and n + j + 1 or j
end

utf8.len = utf8.len or function(s, i, j)
    i, j = bounds(s, i, j)
    local cnt, pos = 0, i
    while pos <= j do
        local nxt = dec(s, pos) if not nxt then return nil, pos end
        pos, cnt = nxt, cnt + 1
    end
    return cnt
end

utf8.char = utf8.char or function(...)
    local r, n = {}, select("#", ...)
    for i = 1, n do
        local v = select(i, ...)
        r[i] = v < 128 and c(v)
            or v < 2048 and c(192 + f(v / 64), 128 + (v % 64))
            or v < 65536 and c(224 + f(v / 4096), 128 + (f(v / 64) % 64), 128 + (v % 64))
            or v < 1114112 and c(240 + f(v / 262144), 128 + (f(v / 4096) % 64), 128 + (f(v / 64) % 64), 128 + (v % 64))
            or error("bad code")
    end
    return table.concat(r)
end

utf8.codepoint = utf8.codepoint or function(s, i, j)
    i, j = bounds(s, i, j or (i and i or #s))
    local r, pos, idx = {}, i, 1
    while pos <= j do
        local nxt, cp = dec(s, pos) if not nxt then error("err") end
        r[idx], idx, pos = cp, idx + 1, nxt
    end
    return unp(r)
end

utf8.codes = utf8.codes or function(s)
    local pos = 1
    return function()
        if pos > #s then return nil end
        local curr = pos
        local nxt, cp = dec(s, pos) if not nxt then error("err") end
        pos = nxt
        return curr, cp
    end
end

return { utf8 = utf8, math = math, table = table }