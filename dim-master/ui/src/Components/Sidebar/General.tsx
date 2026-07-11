import { NavLink } from "react-router-dom";

import HomeIcon from "../../assets/Icons/Home";
import NotificationBell from "../NotificationBell/Index";

const General = () => (
  <section className="yourAccount">
    <header>
      <h4>General</h4>
      <NotificationBell />
    </header>
    <div className="list">
      <NavLink className="item" to="/" exact>
        <HomeIcon />
        <p>Dashboard</p>
      </NavLink>
    </div>
  </section>
);

export default General;
