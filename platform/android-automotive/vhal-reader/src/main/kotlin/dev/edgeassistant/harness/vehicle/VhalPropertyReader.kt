package dev.edgeassistant.harness.vehicle

import android.car.VehiclePropertyManager
import android.car.VehiclePropertyIds
import android.car.hardware.property.VehiclePropertyKey

/** Read request produced by the Rust VHAL read gateway. */
data class VhalReadRequest(
    val propertyName: String,
    val areaId: Int,
)

/** Read result returned to the Rust VHAL read gateway. */
data class VhalReadResult(
    val value: String,
)

/** Platform property reader contract mirrored from `harness-vehicle`. */
interface VehiclePropertyReader {
    fun read(request: VhalReadRequest): VhalReadResult
}

/**
 * Thin Android Automotive adapter.
 *
 * Policy (whitelist, mode, rate limit, capability) is enforced by the Rust
 * gateway. This class only reads a mapped property and serializes its value.
 */
class AndroidVehiclePropertyReader(
    private val propertyManager: VehiclePropertyManager,
) : VehiclePropertyReader {

    private fun propertyIdFor(propertyName: String): Int =
        when (propertyName) {
            "vehicle.speed" -> VehiclePropertyIds.PERFORMANCE_VEHICLE_SPEED
            "vehicle.gear" -> VehiclePropertyIds.CURRENT_GEAR
            "vehicle.battery_level" -> VehiclePropertyIds.ENERGY_LEVEL
            "vehicle.charging_status" -> VehiclePropertyIds.CHARGING_STATUS
            "cabin.temperature" -> error(
                "cabin.temperature must be mapped to a target AAOS property",
            )
            else -> error("unknown vehicle property: $propertyName")
        }

    override fun read(request: VhalReadRequest): VhalReadResult {
        val propertyId = propertyIdFor(request.propertyName)
        val key = VehiclePropertyKey(propertyId, request.areaId)
        val status = propertyManager.getProperty(key)
        return VhalReadResult(serializeValue(status.value))
    }

    private fun serializeValue(value: Any?): String =
        when (value) {
            is Float -> value.toString()
            is Int -> value.toString()
            is Boolean -> value.toString()
            is List<*> -> value.joinToString(",") { serializeValue(it) }
            null -> "null"
            else -> value.toString()
        }
}
