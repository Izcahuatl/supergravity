local use_key = keybinds:fromVanilla("key.use")
local pat_callbacks = {}

function onPat(id, callback)
    pat_callbacks[id] = callback
end

avatar:store("petpet", function(shitter_uuid, time)
    for _, callback in pairs(pat_callbacks) do
        pcall(callback, shitter_uuid, time or 10)
    end
    return true, true
end)

if host:isHost() then
    events.TICK:register(function()
        if not player or not player:isLoaded() or host:getScreen() or action_wheel:isEnabled() then
            return
        end
        if use_key:isPressed() and player:isCrouching() then
            local eye_pos = player:getPos() + vec(0, player:getEyeHeight(), 0)
            local look_dir = player:getLookDir()
            local target = raycast:entity(eye_pos, eye_pos + look_dir * 4.0, function(entity)
                return entity:isPlayer() and entity ~= player
            end)
            if target then
                local target_uuid = target:getUUID()
                local target_vars = target:getVariable()
                local petpet = target_vars and target_vars["petpet"]
                if petpet then
                    pcall(petpet, player:getUUID(), 10)
                    host:swingArm()
                    pings.miniPat(target_uuid)
                end
            end
        end
    end)
end

function pings.miniPat(target_uuid)
    if host:isHost() then
        return
    end
    local target = world.getEntity(target_uuid)
    if target then
        local target_vars = target:getVariable()
        local petpet = target_vars and target_vars["petpet"]
        if petpet then
            pcall(petpet, player:getUUID(), 10)
        end
    end
end