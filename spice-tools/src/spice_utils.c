#include "spice_utils.h"
#include <stdio.h>

static int handle_spice_error(char *error, size_t error_size) {
    if (!failed_c()) {
        if (error_size > 0) error[0] = '\0';
        return 0;
    }

    SpiceChar short_message[81];
    SpiceChar long_message[1841];
    getmsg_c("SHORT", sizeof(short_message), short_message);
    getmsg_c("LONG", sizeof(long_message), long_message);
    snprintf(error, error_size, "%s: %s", short_message, long_message);
    reset_c();
    return -1;
}

int get_gravitational_param(ConstSpiceChar *body_name, SpiceDouble *gm, char *error, size_t error_size) {
    SpiceInt dim;
    erract_c("SET", 0, "RETURN");
    errprt_c("SET", 0, "NONE");
    bodvrd_c(body_name, "GM", 1, &dim, gm);
    return handle_spice_error(error, error_size);
}

int state2keplerian(const SpiceDouble state[6], SpiceDouble et, SpiceDouble gm, SpiceDouble elements[6], char *error, size_t error_size) {
    SpiceDouble elts[8];
    erract_c("SET", 0, "RETURN");
    errprt_c("SET", 0, "NONE");
    oscelt_c(state, et, gm, elts);

    if (handle_spice_error(error, error_size) != 0) return -1;

    elements[0] = elts[0]; // Semi-major axis (km)
    elements[1] = elts[1]; // Eccentricity
    elements[2] = elts[2]; // Inclination (rad)
    elements[3] = elts[3]; // Longitude of ascending node (rad)
    elements[4] = elts[4]; // Argument of periapsis (rad)
    elements[5] = elts[5]; // Mean anomaly at epoch (rad)
    return 0;
}

int utc2et(const char *utc, SpiceDouble *et, char *error, size_t error_size) {
    erract_c("SET", 0, "RETURN");
    errprt_c("SET", 0, "NONE");
    utc2et_c(utc, et);
    return handle_spice_error(error, error_size);
}

int jd_to_et(SpiceDouble jd, SpiceDouble *et, char *error, size_t error_size) {
    erract_c("SET", 0, "RETURN");
    errprt_c("SET", 0, "NONE");
    *et = unitim_c(jd, "JED", "ET");
    return handle_spice_error(error, error_size);
}

int timout(SpiceDouble et, const char *pictur, size_t lenout, char *output, char *error, size_t error_size) {
    erract_c("SET", 0, "RETURN");
    errprt_c("SET", 0, "NONE");
    timout_c(et, pictur, lenout, output);
    return handle_spice_error(error, error_size);
}

int get_current_utc(char *utc_str, size_t size) {
    time_t rawtime;
    struct tm *timeinfo;

    time(&rawtime);
    timeinfo = gmtime(&rawtime);
    return strftime(utc_str, size, "%Y-%m-%d %H:%M:%S", timeinfo) == 0 ? -1 : 0;
}

int spkez(SpiceDouble et, SpiceInt target, SpiceInt observer, const char* ref_frame, SpiceDouble state[6], char *error, size_t error_size) {
    SpiceDouble lt;
    erract_c("SET", 0, "RETURN");
    errprt_c("SET", 0, "NONE");
    spkez_c(target, et, ref_frame, "NONE", observer, state, &lt);
    return handle_spice_error(error, error_size);
}

int bodc2n(SpiceInt obj_id, char* obj_name, size_t obj_name_size, SpiceBoolean *found, char *error, size_t error_size) {
    erract_c("SET", 0, "RETURN");
    errprt_c("SET", 0, "NONE");
    bodc2n_c(obj_id, obj_name_size, obj_name, found);
    return handle_spice_error(error, error_size);
}

int bodvcd_radii(SpiceInt id, SpiceDouble radii[3], char *error, size_t error_size) {
    SpiceInt n;
    erract_c("SET", 0, "RETURN");
    errprt_c("SET", 0, "NONE");
    bodvcd_c(id, "RADII", 3, &n, radii);
    return handle_spice_error(error, error_size);
}

int bodvcd_mu(SpiceInt id, SpiceDouble* mu, char *error, size_t error_size) {
    SpiceInt n;
    erract_c("SET", 0, "RETURN");
    errprt_c("SET", 0, "NONE");
    bodvcd_c(id, "GM", 1, &n, mu);
    return handle_spice_error(error, error_size);
}

int load_kernel(const char *kernel_path, char *error, size_t error_size) {
    erract_c("SET", 0, "RETURN");
    errprt_c("SET", 0, "NONE");
    furnsh_c(kernel_path);
    return handle_spice_error(error, error_size);
}

int unload_kernel(const char *kernel_path, char *error, size_t error_size) {
    erract_c("SET", 0, "RETURN");
    errprt_c("SET", 0, "NONE");
    unload_c(kernel_path);
    return handle_spice_error(error, error_size);
}

int clear_spice(char *error, size_t error_size) {
    erract_c("SET", 0, "RETURN");
    errprt_c("SET", 0, "NONE");
    kclear_c();
    return handle_spice_error(error, error_size);
}
