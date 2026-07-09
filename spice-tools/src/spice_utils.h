#ifndef SPICE_UTILS_H
#define SPICE_UTILS_H

#include "../cspice/include/SpiceUsr.h"
#include <time.h>

int get_gravitational_param(ConstSpiceChar *body_name, SpiceDouble *gm, char *error, size_t error_size);

int state2keplerian(const SpiceDouble state[6], SpiceDouble et, SpiceDouble gm, SpiceDouble elements[6], char *error, size_t error_size);

int utc2et(const char *utc, SpiceDouble *et, char *error, size_t error_size);

int jd_to_et(SpiceDouble jd, SpiceDouble *et, char *error, size_t error_size);

int timout(SpiceDouble et, const char *pictur, size_t lenout, char *output, char *error, size_t error_size);

int get_current_utc(char *utc_str, size_t size);

int spkez(SpiceDouble et, SpiceInt target, SpiceInt observer, const char* ref_frame, SpiceDouble state[6], char *error, size_t error_size);

int bodc2n(SpiceInt obj_id, char* obj_name, size_t obj_name_size, SpiceBoolean *found, char *error, size_t error_size);

int bodvcd_radii(SpiceInt id, SpiceDouble radii[3], char *error, size_t error_size);
int bodvcd_mu(SpiceInt id, SpiceDouble* mu, char *error, size_t error_size);

int load_kernel(const char *kernel_path, char *error, size_t error_size);
int unload_kernel(const char *kernel_path, char *error, size_t error_size);
int clear_spice(char *error, size_t error_size);

#endif // SPICE_UTILS_H
